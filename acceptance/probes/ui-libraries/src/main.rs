#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use uiautomation::{
        UIAutomation, UIElement,
        patterns::{UIInvokePattern, UIValuePattern},
    };
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    // Probe process owns all COM objects on its single MTA thread.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }
    let _apartment = Apartment;
    let automation = UIAutomation::new_direct()?;
    let args: Vec<_> = std::env::args().collect();
    let root = automation.element_from_handle(args[1].parse::<isize>()?.into())?;
    let walker = automation.get_control_view_walker()?;
    let mut stack = vec![root];
    let mut entries: Vec<UIElement> = vec![];
    while let Some(element) = stack.pop() {
        if entries.len() >= 100 {
            return Err("fixture unexpectedly large".into());
        }
        if let Ok(mut child) = walker.get_first_child(&element) {
            loop {
                stack.push(child.clone());
                match walker.get_next_sibling(&child) {
                    Ok(next) => child = next,
                    Err(_) => break,
                }
                if stack.len() > 100 {
                    return Err("fixture sibling budget".into());
                }
            }
        }
        entries.push(element);
    }
    if args.get(2).is_some_and(|mode| mode == "inspect") {
        for element in &entries {
            println!("{:?} {:?}", element.get_name(), element.get_control_type());
        }
    }
    if args.get(2).is_some_and(|mode| mode == "act") {
        let input = entries
            .iter()
            .find(|e| e.get_name().ok().as_deref() == Some("Fixture input"))
            .ok_or("input absent")?;
        let value: UIValuePattern = input.get_pattern()?;
        value.set_value("PAB 中文🙂")?;
        assert_eq!(value.get_value()?, "PAB 中文🙂");
        let button = entries
            .iter()
            .find(|e| e.get_name().ok().as_deref() == Some("Apply fixture"))
            .ok_or("button absent")?;
        let invoke: UIInvokePattern = button.get_pattern()?;
        invoke.invoke()?;
    }
    println!(
        "{}",
        serde_json::json!({"nodes":entries.len(), "mode":args.get(2), "library":"uiautomation 0.25.1"})
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let outcome = if std::env::args().nth(2).as_deref() == Some("child_inspect") {
        mac_child_probe()
    } else {
        mac_probe()
    };
    if let Some(path) = std::env::args().nth(3) {
        std::fs::write(
            path,
            serde_json::to_vec(
                &serde_json::json!({"success":outcome.is_ok(),"error":outcome.as_ref().err().map(|e|e.to_string())}),
            )?,
        )?;
    }
    outcome
}

// Verify a signed application can delegate AX calls to its own disposable child.
#[cfg(target_os = "macos")]
fn mac_child_probe() -> Result<(), Box<dyn std::error::Error>> {
    use std::{
        process::Command,
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args().collect();
    let mut child = Command::new(std::env::current_exe()?)
        .args([&args[1], "inspect"])
        .spawn()?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err("AX child failed".into())
            };
        }
        if start.elapsed() >= Duration::from_secs(2) {
            child.kill()?;
            child.wait()?;
            return Err("AX child deadline: killed and reaped".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "macos")]
fn mac_probe() -> Result<(), Box<dyn std::error::Error>> {
    use accessibility::{AXAttribute, AXUIElement, AXUIElementAttributes};
    use core_foundation::{base::TCFType, string::CFString};
    let args: Vec<_> = std::env::args().collect();
    let app = AXUIElement::application(args[1].parse()?);
    app.set_messaging_timeout(0.5)?;
    app.role()?; // Permission/provider failures must not look like an empty successful tree.
    let mut stack: Vec<AXUIElement> = app.windows()?.iter().map(|e| (*e).clone()).collect();
    if stack
        .iter()
        .any(|e| e.role().ok().map(|s| s.to_string()).as_deref() != Some("AXWindow"))
    {
        return Err("AXWindows contained a non-window root".into());
    }
    let mut entries: Vec<AXUIElement> = Vec::new();
    while let Some(element) = stack.pop() {
        if entries.contains(&element) {
            continue;
        }
        if entries.len() >= 100 {
            return Err("fixture unexpectedly large".into());
        }
        element.set_messaging_timeout(0.5)?;
        if let Ok(children) = element.children() {
            if children.len() > 100 {
                return Err("fixture sibling budget".into());
            }
            for child in children.iter() {
                stack.push((*child).clone());
            }
        }
        entries.push(element);
    }
    let title = |e: &AXUIElement| {
        e.title()
            .ok()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| e.description().ok().map(|s| s.to_string()))
            .unwrap_or_default()
    };
    if args.get(2).is_some_and(|mode| mode == "inspect") {
        for element in &entries {
            println!(
                "{} {:?}",
                title(element),
                element.role().map(|s| s.to_string())
            );
        }
    }
    if args.get(2).is_some_and(|mode| mode == "act") {
        let input = entries
            .iter()
            .find(|e| title(e) == "Fixture input")
            .ok_or("input absent")?;
        input.set_attribute(
            &AXAttribute::value(),
            CFString::new("PAB 中文🙂").as_CFType(),
        )?;
        assert_eq!(
            input
                .value()?
                .downcast::<CFString>()
                .ok_or("nontext value")?
                .to_string(),
            "PAB 中文🙂"
        );
        let button = entries
            .iter()
            .find(|e| title(e) == "Apply fixture")
            .ok_or("button absent")?;
        button.perform_action(&CFString::new("AXPress"))?;
    }
    println!(
        "{}",
        serde_json::json!({"nodes":entries.len(),"library":"accessibility 0.2.0"})
    );
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("Unsupported probe platform");
    std::process::exit(2);
}
