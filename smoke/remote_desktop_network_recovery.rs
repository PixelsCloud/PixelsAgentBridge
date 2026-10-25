//! Real QUIC through a counted, bounded UDP proxy on loopback. Synthetic
//! transport payloads only: this does not qualify native H.264 or App recovery.
use super::*;
use std::{
    collections::VecDeque,
    net::{Ipv4Addr, SocketAddr},
    sync::atomic::AtomicBool,
};
use tokio::{net::UdpSocket, task::JoinHandle};

struct Proxy {
    address: SocketAddr,
    blocked: Arc<AtomicBool>,
    every: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
    forwarded: Arc<AtomicUsize>,
    overflow: Arc<AtomicUsize>,
    bypassed: Arc<AtomicBool>,
    monitor: Option<JoinHandle<()>>,
    task: JoinHandle<()>,
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(task) = &self.monitor {
            task.abort();
        }
    }
}
impl Proxy {
    async fn start(target: SocketAddr, delay: Duration) -> Self {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = socket.local_addr().unwrap();
        let blocked = Arc::new(AtomicBool::new(false));
        let every = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let forwarded = Arc::new(AtomicUsize::new(0));
        let overflow = Arc::new(AtomicUsize::new(0));
        let (block, each, drops, sent, full) = (
            blocked.clone(),
            every.clone(),
            dropped.clone(),
            forwarded.clone(),
            overflow.clone(),
        );
        let task = tokio::spawn(async move {
            let mut controller = None;
            let mut buffer = vec![0; 65536];
            let mut pending: VecDeque<(tokio::time::Instant, SocketAddr, Vec<u8>)> =
                VecDeque::new();
            let mut sequence = 0usize;
            loop {
                let next = pending
                    .front()
                    .map(|p| p.0)
                    .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(60));
                tokio::select! {
                    _=tokio::time::sleep_until(next), if !pending.is_empty()=> {
                        while pending.front().is_some_and(|p|p.0<=tokio::time::Instant::now()) {
                            let (_,to,data)=pending.pop_front().unwrap();
                            socket.send_to(&data,to).await.unwrap();sent.fetch_add(1,Ordering::SeqCst);
                        }
                    }
                    packet=socket.recv_from(&mut buffer)=> {
                        let (n,from)=packet.unwrap();
                        let to=if from==target {if let Some(c)=controller {c}else{continue}}
                            else if controller==Some(from) {target}
                            else if controller.is_none() && from.ip().is_loopback() {controller=Some(from);target}
                            else {continue};
                        sequence+=1;
                        let period=each.load(Ordering::SeqCst);
                        if block.load(Ordering::SeqCst) || (period>0 && sequence%period==0) {
                            drops.fetch_add(1,Ordering::SeqCst);continue;
                        }
                        if pending.len()==256 {full.fetch_add(1,Ordering::SeqCst);continue;}
                        pending.push_back((tokio::time::Instant::now()+delay,to,buffer[..n].to_vec()));
                    }
                }
            }
        });
        Self {
            address,
            blocked,
            every,
            dropped,
            forwarded,
            overflow,
            bypassed: Arc::new(AtomicBool::new(false)),
            monitor: None,
            task,
        }
    }
    fn selected(&self, server: &Harness) {
        assert!(
            !self.bypassed.load(Ordering::SeqCst),
            "a sampled path bypassed the impairment proxy"
        );
        let expected = format!("ip:{}", self.address);
        assert_eq!(
            server.client.statistics().unwrap().remote_addr,
            expected,
            "client bypassed the impairment proxy"
        );
        assert_eq!(
            server.peer.statistics().unwrap().remote_addr,
            expected,
            "host bypassed the impairment proxy"
        );
        assert_eq!(
            self.overflow.load(Ordering::SeqCst),
            0,
            "proxy queue overflow invalidated the impairment"
        );
    }
    fn monitor(&mut self, server: &Harness) {
        let (client, host) = (server.client.clone(), server.peer.clone());
        let expected = format!("ip:{}", self.address);
        let bypassed = self.bypassed.clone();
        self.monitor = Some(tokio::spawn(async move {
            loop {
                for peer in [&client, &host] {
                    if peer.statistics().is_some_and(|s| s.remote_addr != expected) {
                        bypassed.store(true, Ordering::SeqCst);
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }));
    }
}

async fn proxied(global: &DesktopHost) -> (Harness, Proxy) {
    let config = PabEndpointConfig::new(vec!["https://127.0.0.1:1".parse().unwrap()])
        .unwrap()
        .counted_udp_proxy_for_testing();
    let a = PabEndpoint::bind(config.clone(), iroh_base::SecretKey::generate())
        .await
        .unwrap();
    let b = PabEndpoint::bind(config, iroh_base::SecretKey::generate())
        .await
        .unwrap();
    let mut addresses = b.watch_address();
    let port = tokio::time::timeout(WAIT, async {
        loop {
            if let Some(port) = addresses
                .borrow()
                .direct_addresses
                .iter()
                .find(|s| s.is_ipv4())
                .map(|s| s.port())
            {
                break port;
            }
            addresses.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let mut proxy = Proxy::start(
        (Ipv4Addr::LOCALHOST, port).into(),
        Duration::from_millis(20),
    )
    .await;
    let remote = b.clone();
    let accept = tokio::spawn(async move { remote.accept().await.unwrap().unwrap() });
    let client = a
        .connect(
            *b.id().as_bytes(),
            &PabEndpointAddress {
                direct_addresses: vec![proxy.address],
                relay_urls: vec![],
            },
            WAIT,
        )
        .await
        .unwrap();
    let server = Harness::with_pair(global, (a, b, client, accept.await.unwrap())).await;
    proxy.selected(&server);
    proxy.monitor(&server);
    (server, proxy)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn counted_udp_blackout_recovers_same_video_without_persistent_throttling() {
    let global = DesktopHost::new(Some(Arc::new(Backend {
        frame_bytes: Some(16 * 1024),
        ..Default::default()
    })));
    let (server, proxy) = proxied(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    let session = viewer.opened().config.session_id;
    let mut last = 0;
    for _ in 0..12 {
        let f = viewer.next_frame(WAIT).await.unwrap();
        last = f.header.frame_id;
        viewer.report_presented(&f.header).unwrap();
    }
    proxy.blocked.store(true, Ordering::SeqCst);
    let restore = proxy.blocked.clone();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        restore.store(false, Ordering::SeqCst);
    });
    let mut gap = Duration::ZERO;
    for _ in 0..12 {
        let at = tokio::time::Instant::now();
        let f = viewer.next_frame(WAIT).await.unwrap();
        gap = gap.max(at.elapsed());
        assert_eq!(f.header.frame_id, last + 1);
        last = f.header.frame_id;
        assert!(f.bytes.iter().all(|b| *b == 0xaa));
        viewer.report_presented(&f.header).unwrap();
    }
    release.await.unwrap();
    assert!(
        gap >= Duration::from_millis(450),
        "blackout never interrupted media: {gap:?}"
    );
    assert!(
        proxy.dropped.load(Ordering::SeqCst) > 0,
        "no actual datagrams were dropped"
    );
    let recovered = tokio::time::Instant::now();
    for _ in 0..24 {
        let f = viewer.next_frame(WAIT).await.unwrap();
        assert_eq!(f.header.frame_id, last + 1);
        last = f.header.frame_id;
        viewer.report_presented(&f.header).unwrap();
    }
    assert!(
        recovered.elapsed() < Duration::from_millis(1900),
        "post-outage cadence retained a stale penalty"
    );
    assert_eq!(viewer.opened().config.session_id, session);
    assert_eq!(viewer.opened().config.generation, 1);
    assert_eq!(
        viewer
            .quality_control()
            .snapshot()
            .await
            .unwrap()
            .capture_fps_milli,
        15_000
    );
    proxy.selected(&server);
    server.legacy().await;
    println!(
        "blackout_ms=900 max_read_wait_ms={} dropped={} forwarded={}",
        gap.as_millis(),
        proxy.dropped.load(Ordering::SeqCst),
        proxy.forwarded.load(Ordering::SeqCst)
    );
    viewer.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn counted_sparse_loss_and_delay_preserve_order_and_control_stream() {
    let global = DesktopHost::new(Some(Arc::new(Backend {
        frame_bytes: Some(16 * 1024),
        ..Default::default()
    })));
    let (server, proxy) = proxied(&global).await;
    let mut viewer = RemoteDesktopSession::open(server.client.clone(), None, 15, WAIT)
        .await
        .unwrap();
    proxy.every.store(50, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(15), async {
        for id in 1..=45 {
            let f = viewer.next_frame(WAIT).await.unwrap();
            assert_eq!(f.header.frame_id, id);
            assert!(f.bytes.iter().all(|b| *b == 0xaa));
            viewer.report_presented(&f.header).unwrap();
        }
        server.legacy().await;
    })
    .await
    .unwrap();
    proxy.selected(&server);
    assert!(proxy.dropped.load(Ordering::SeqCst) > 0);
    println!(
        "per_hop_delay_ms=20 drop_every=50 dropped={} forwarded={}",
        proxy.dropped.load(Ordering::SeqCst),
        proxy.forwarded.load(Ordering::SeqCst)
    );
    viewer.close().await.unwrap();
}
