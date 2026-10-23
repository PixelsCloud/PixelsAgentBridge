export type PipelinePerformance = {
  session_id: string; generation: number; visible: boolean; network_rtt_ms: number | null;
  connection_path: string | null; remote_age_ms: number | null;
  local: {
    received_frame_id: number; submitted_frame_id: number; presented_frame_id: number;
    receive_fps_milli: number; present_fps_milli: number; bitrate_bps: number;
    last_received_ms: number | null; last_presented_ms: number | null;
    pending_frames: number; oldest_pending_ms: number | null;
    submit_last_us: number | null; receive_to_present_last_us: number | null; stage: string;
  };
  remote: null | {
    stage: string; stage_age_ms: number; produced_frames: number; sent_frame_id: number;
    presented_frame_id: number; capture: null | { wait_us: number; encode_us: number };
    source_last_us: number | null; send_last_us: number | null; presentation_ack_last_us: number | null;
    oldest_unpresented_ms: number | null;
  };
};
export function pipelineRows(p: PipelinePerformance, english: boolean): string[][] {
  const unknown = english ? "Not reported" : "未上报";
  const ms = (value: number | null | undefined) => value == null ? unknown : `${value.toFixed(1)} ms`;
  const us = (value: number | null | undefined) => ms(value == null ? null : value / 1000);
  const fresh = p.remote_age_ms != null && p.remote_age_ms <= 6000;
  const host = fresh ? p.remote : null;
  const localStage: Record<string, string> = english
    ? { submitting: "Submitting to decoder", presentation: "Waiting for presentation", first_frame: "Waiting for first frame", idle: "Waiting for changed frames" }
    : { submitting: "送入解码器", presentation: "等待解码显示", first_frame: "等待首帧", idle: "等待画面变化" };
  const hostStage: Record<string, string> = english
    ? { starting: "Starting", credit: "Pacing / presentation credit", capture: "Capture / encode (may wait for changes)", send: "Writing video" }
    : { starting: "启动", credit: "限帧／等待显示确认", capture: "采集编码（可能等待变化）", send: "发送画面" };
  const local = p.local;
  return [
    [english ? "Pipeline" : "流水线进度", `${localStage[local.stage] || unknown}${p.visible ? "" : english ? " (hidden)" : "（窗口隐藏）"}`],
    [english ? "Measured receive / display" : "实测接收／显示", `${(local.receive_fps_milli / 1000).toFixed(1)} / ${(local.present_fps_milli / 1000).toFixed(1)} FPS · ${(local.bitrate_bps / 1_000_000).toFixed(2)} Mbps`],
    [english ? "QUIC RTT / video write" : "网络往返／视频写入", `${ms(p.network_rtt_ms)} / ${us(host?.send_last_us)}`],
    [english ? "Capture wait / encode" : "采集等待／编码", `${us(host?.capture?.wait_us)} / ${us(host?.capture?.encode_us)}`],
    [english ? "Decoder submit / receive to display" : "解码提交／接收到显示", `${us(local.submit_last_us)} / ${us(local.receive_to_present_last_us)}`],
    [english ? "Receive / submit / display IDs" : "接收／提交／显示帧号", `${local.received_frame_id} / ${local.submitted_frame_id} / ${local.presented_frame_id}`],
    [english ? "Pending / oldest" : "待显示／最久等待", `${local.pending_frames} / ${ms(local.oldest_pending_ms)}`],
    [english ? "Producer / sent / ACK" : "被控端／发送／确认", host ? `${hostStage[host.stage] || unknown} · ${host.sent_frame_id} / ${host.presented_frame_id}` : unknown],
  ];
}
