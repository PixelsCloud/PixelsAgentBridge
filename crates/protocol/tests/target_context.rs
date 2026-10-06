use pab_protocol::{
    ContextFreshness, CpuArchitecture, DeviceId, DeviceRef, ExecutionContext, ExecutionScope,
    ExpectedEnvironment, InterpreterContext, OsFamily, PathStyle, TargetContext,
    TargetContextSource, TenantId,
};

#[test]
fn historical_context_without_account_remains_unobserved() {
    let value = serde_json::json!({
        "os_family":"linux","os_name":"Linux","os_version":"test",
        "architecture":"x86_64","execution_scope":"native","path_style":"posix",
        "interpreter":null,"cwd":"/tmp","environment_revision":"old-env"
    });
    let context: ExecutionContext = serde_json::from_value(value.clone()).unwrap();
    assert!(context.identity.is_none());
    assert_eq!(serde_json::to_value(context).unwrap(), value);
}

#[test]
fn compact_reminder_keeps_target_os_shell_and_revision_visible() {
    let context = TargetContext {
        device_ref: DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        },
        execution: ExecutionContext {
            os_family: OsFamily::Windows,
            os_name: "Windows Server".to_owned(),
            os_version: "2025".to_owned(),
            architecture: CpuArchitecture::X86_64,
            execution_scope: ExecutionScope::Native,
            identity: None,
            path_style: PathStyle::Windows,
            interpreter: Some(InterpreterContext {
                id: "windows_powershell".to_owned(),
                name: "Windows PowerShell".to_owned(),
                version: "5.1".to_owned(),
                executable_path: r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
                    .to_owned(),
            }),
            cwd: Some(r"C:\Work".to_owned()),
            environment_revision: "env-7".to_owned(),
        },
        source: TargetContextSource::HistoricalTask,
        observed_at_unix_ms: 1_000,
        freshness: ContextFreshness::Historical,
    };

    let reminder = context.compact_reminder();
    assert!(reminder.contains("Windows x86_64"));
    assert!(reminder.contains("Windows PowerShell 5.1"));
    assert!(reminder.contains(r"cwd=C:\Work"));
    assert!(reminder.contains("env=env-7"));
}

#[test]
fn expected_environment_requires_both_os_and_revision() {
    let execution = ExecutionContext {
        os_family: OsFamily::Linux,
        os_name: "Linux".to_owned(),
        os_version: "test".to_owned(),
        architecture: CpuArchitecture::Aarch64,
        execution_scope: ExecutionScope::Native,
        identity: None,
        path_style: PathStyle::Posix,
        interpreter: None,
        cwd: Some("/tmp/line\nname".to_owned()),
        environment_revision: "env-2".to_owned(),
    };
    assert!(execution.matches_expected(&ExpectedEnvironment {
        os_family: OsFamily::Linux,
        environment_revision: "env-2".to_owned(),
    }));
    assert!(!execution.matches_expected(&ExpectedEnvironment {
        os_family: OsFamily::Linux,
        environment_revision: "env-1".to_owned(),
    }));

    let context = TargetContext {
        device_ref: DeviceRef {
            tenant_id: TenantId::from_u128(2),
            device_id: DeviceId::from_u128(3),
        },
        execution,
        source: TargetContextSource::ExecutorVerified,
        observed_at_unix_ms: 1,
        freshness: ContextFreshness::Current,
    };
    let reminder = context.compact_reminder();
    assert!(reminder.contains(r"cwd=/tmp/line\nname"));
    assert!(!reminder.contains('\n'));
}
