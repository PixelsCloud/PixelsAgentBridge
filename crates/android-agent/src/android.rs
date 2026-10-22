//! JNI contains panics and returns fixed error codes without logging credentials.

use crate::{
    config::{AndroidConfig, ControlIntent, MAX_CONFIG_BYTES},
    control::ControlAgent,
};
use jni::{
    JNIEnv,
    objects::{JByteArray, JObject, JString},
    sys::{jboolean, jlong, jstring},
};
use serde_json::{Value, json};
use std::sync::Mutex;

static AGENT: Mutex<Option<ControlAgent>> = Mutex::new(None);

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_renameDevice(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    toml: JString<'_>,
    identity: JByteArray<'_>,
    registration: JString<'_>,
    name: JString<'_>,
) -> jstring {
    reply(env, |env| {
        let config = AndroidConfig::parse(&string(env, &toml, MAX_CONFIG_BYTES)?)?;
        let identity = key(env, &identity)?;
        ControlAgent::rename_device(
            &config,
            &identity,
            &string(env, &registration, 4096)?,
            &string(env, &name, 1024)?,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_updateAccessPassword(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    hash: JString<'_>,
    version: jlong,
) -> jstring {
    reply(env, |env| {
        let version = u64::try_from(version).map_err(|_| "invalid_access_password")?;
        let credential =
            pab_device_session::DevicePassword::from_phc(version, string(env, &hash, 1024)?)
                .map_err(|_| "invalid_access_password")?;
        if let Some(agent) = AGENT.lock().map_err(|_| "lifecycle_unavailable")?.as_ref() {
            agent.update_password(credential)?;
        }
        Ok(json!({"updated":true}))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_recentActivity(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    directory: JString<'_>,
) -> jstring {
    reply(env, |env| {
        let directory = string(env, &directory, 4096)?;
        let path = std::path::Path::new(&directory);
        if !path.is_absolute()
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| {
                    name.len() != 64 || !name.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        {
            return Err("invalid_activity_directory");
        }
        // No AGENT mutex is held while reading. This shares bundled SQLite's VFS
        // lock bookkeeping with the journal writers, including WAL checkpoints.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "activity_unavailable")?;
        Ok(runtime.block_on(crate::activity::read(path)))
    })
}

/// Atomic publication runs as the app UID. No shell, elevated permission or hard link.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_publishFile(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    source: JString<'_>,
    destination: JString<'_>,
    replace: jboolean,
) -> jstring {
    reply(env, |env| {
        let source = string(env, &source, 4096)?;
        let destination = string(env, &destination, 4096)?;
        if !std::path::Path::new(&source).is_absolute()
            || !std::path::Path::new(&destination).is_absolute()
            || source == destination
        {
            return Err("invalid_path");
        }
        let source_c = std::ffi::CString::new(source).map_err(|_| "invalid_path")?;
        let destination_c =
            std::ffi::CString::new(destination.clone()).map_err(|_| "invalid_path")?;
        let flags: u32 = if replace == 0 { 1 } else { 0 }; // Linux RENAME_NOREPLACE.
        // SAFETY: Both NUL-terminated strings remain alive through the synchronous
        // syscall. AT_FDCWD takes no owned descriptor. syscall avoids a Bionic
        // renameat2 symbol dependency on the minimum supported Android version.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                source_c.as_ptr(),
                libc::AT_FDCWD,
                destination_c.as_ptr(),
                flags,
            )
        };
        if result != 0 {
            return Err(match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EEXIST | libc::ENOTEMPTY) => "already_exists",
                Some(libc::EACCES | libc::EPERM) => "access_denied",
                Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP | libc::EXDEV) => {
                    "atomic_publish_unavailable"
                }
                _ => "storage_io_error",
            });
        }
        let parent = std::path::Path::new(&destination)
            .parent()
            .ok_or("publication_unconfirmed")?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| "publication_unconfirmed")?;
        Ok(json!({"published":true}))
    })
}

fn reply<'local>(
    mut env: JNIEnv<'local>,
    operation: impl FnOnce(&mut JNIEnv<'local>) -> Result<Value, &'static str>,
) -> jstring {
    let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(&mut env)))
        .unwrap_or(Err("native_panic"));
    let result = match value {
        Ok(value) => json!({"ok":true,"value":value}),
        Err(code) => json!({"ok":false,"error":code}),
    };
    env.new_string(result.to_string())
        .map(|text| text.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn string(env: &mut JNIEnv<'_>, value: &JString<'_>, max: usize) -> Result<String, &'static str> {
    let text: String = env.get_string(value).map_err(|_| "invalid_string")?.into();
    if text.len() > max {
        return Err("input_too_large");
    }
    Ok(text)
}

fn key(
    env: &mut JNIEnv<'_>,
    value: &JByteArray<'_>,
) -> Result<zeroize::Zeroizing<Vec<u8>>, &'static str> {
    if env
        .get_array_length(value)
        .map_err(|_| "invalid_identity")?
        != 32
    {
        return Err("invalid_identity");
    }
    Ok(zeroize::Zeroizing::new(
        env.convert_byte_array(value)
            .map_err(|_| "invalid_identity")?,
    ))
}

fn settings(config: AndroidConfig) -> Result<Value, &'static str> {
    config.validate()?;
    let toml = toml::to_string_pretty(&config).map_err(|_| "invalid_config")?;
    Ok(json!({"config":config,"toml":toml,"scope":config.credential_scope()}))
}

fn user_intent(intent: ControlIntent) -> Result<Value, &'static str> {
    intent.validate()?;
    let toml = toml::to_string_pretty(&intent).map_err(|_| "invalid_intent")?;
    Ok(json!({"intent":intent,"toml":toml}))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_decodeUserIntent(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    input: JString<'_>,
) -> jstring {
    reply(env, |env| {
        user_intent(ControlIntent::parse(&string(env, &input, 4096)?)?)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_encodeUserIntent(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    input: JString<'_>,
) -> jstring {
    reply(env, |env| {
        let intent =
            serde_json::from_str(&string(env, &input, 4096)?).map_err(|_| "invalid_intent")?;
        user_intent(intent)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_probe(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
) -> jstring {
    let result = std::panic::catch_unwind(super::shared_core_probe)
        .unwrap_or_else(|_| r#"{"status":"error","code":"native_panic"}"#.to_owned());
    // A failed Java allocation leaves the VM exception pending; null is its JNI sentinel.
    env.new_string(result)
        .map(|text| text.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_defaultSettings(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
) -> jstring {
    reply(env, |_| settings(AndroidConfig::defaults()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_encodeSettings(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    input: JString<'_>,
) -> jstring {
    reply(env, |env| {
        let input = string(env, &input, MAX_CONFIG_BYTES)?;
        let config = serde_json::from_str(&input).map_err(|_| "invalid_config")?;
        settings(config)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_decodeSettings(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    input: JString<'_>,
) -> jstring {
    reply(env, |env| {
        settings(AndroidConfig::parse(&string(
            env,
            &input,
            MAX_CONFIG_BYTES,
        )?)?)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_register(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    input: JString<'_>,
    identity: JByteArray<'_>,
) -> jstring {
    reply(env, |env| {
        let config = AndroidConfig::parse(&string(env, &input, MAX_CONFIG_BYTES)?)?;
        ControlAgent::register(&config, &key(env, &identity)?)
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_start(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    input: JString<'_>,
    identity: JByteArray<'_>,
    registration: JString<'_>,
    options: JString<'_>,
) -> jstring {
    reply(env, |env| {
        let config = AndroidConfig::parse(&string(env, &input, MAX_CONFIG_BYTES)?)?;
        let registration = string(env, &registration, 4096)?;
        let options = string(env, &options, 8192)?;
        let options = if options.is_empty() {
            None
        } else {
            Some(serde_json::from_str(&options).map_err(|_| "invalid_runtime_options")?)
        };
        let mut agent = AGENT.lock().map_err(|_| "native_unavailable")?;
        if agent.is_some() {
            return Err("already_connected");
        }
        *agent = Some(ControlAgent::start_with_options(
            &config,
            &key(env, &identity)?,
            &registration,
            options,
        )?);
        Ok(agent.as_ref().expect("started agent").status())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_initialize(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    context: JObject<'_>,
) -> jstring {
    reply(env, |env| {
        static CONTEXT: std::sync::OnceLock<jni::objects::GlobalRef> = std::sync::OnceLock::new();
        if CONTEXT.get().is_some() {
            return Ok(json!({"initialized":true}));
        }
        let global = env
            .new_global_ref(&context)
            .map_err(|_| "context_unavailable")?;
        let vm = env.get_java_vm().map_err(|_| "context_unavailable")?;
        // The process-wide reference owns an Application, never an Activity. Initialize
        // before spawning any Rust networking threads, and retain it until process exit.
        let mut unowned = unsafe { jni22::EnvUnowned::from_raw(env.get_raw().cast()) };
        let outcome = unowned
            .with_env(|env22| {
                let borrowed =
                    unsafe { jni22::objects::JObject::from_raw(env22, context.as_raw().cast()) };
                rustls_platform_verifier::android::init_with_env(env22, borrowed)
            })
            .into_outcome();
        if !matches!(outcome, jni22::Outcome::Ok(())) {
            return Err("tls_context_unavailable");
        }
        unsafe {
            ndk_context::initialize_android_context(
                vm.get_java_vm_pointer().cast(),
                global.as_obj().as_raw().cast(),
            );
        }
        CONTEXT
            .set(global)
            .map_err(|_| "context_already_initialized")?;
        Ok(json!({"initialized":true}))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_passwordHash(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    password: JString<'_>,
) -> jstring {
    reply(env, |env| {
        use argon2::{PasswordHasher, password_hash::SaltString};
        let password = zeroize::Zeroizing::new(string(env, &password, 1024)?);
        if password.len() < 8 {
            return Err("invalid_access_password");
        }
        let salt = SaltString::generate(&mut rand_core::OsRng);
        let hash = argon2::Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|_| "password_hash_failed")?
            .to_string();
        Ok(json!({"hash":hash}))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_pollPlatform(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
) -> jstring {
    reply(env, |_| {
        AGENT
            .lock()
            .map_err(|_| "native_unavailable")?
            .as_ref()
            .ok_or("not_connected")?
            .poll_platform()
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_completePlatform(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    ticket: JString<'_>,
    output: JString<'_>,
    jpeg: JByteArray<'_>,
) -> jstring {
    reply(env, |env| {
        let ticket = string(env, &ticket, 64)?
            .parse()
            .map_err(|_| "invalid_ticket")?;
        let output = serde_json::from_str(&string(env, &output, 48 * 1024)?)
            .map_err(|_| "invalid_platform_result")?;
        if env.get_array_length(&jpeg).map_err(|_| "invalid_image")? > 8 * 1024 * 1024 {
            return Err("invalid_image");
        }
        let jpeg = env.convert_byte_array(jpeg).map_err(|_| "invalid_image")?;
        AGENT
            .lock()
            .map_err(|_| "native_unavailable")?
            .as_ref()
            .ok_or("not_connected")?
            .complete_platform(ticket, output, jpeg)?;
        Ok(json!({"completed":true}))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_status(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
) -> jstring {
    reply(env, |_| {
        let agent = AGENT.lock().map_err(|_| "native_unavailable")?;
        Ok(agent
            .as_ref()
            .map(ControlAgent::status)
            .unwrap_or_else(|| json!({"phase":"disconnected"})))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeBridge_stop(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
) -> jstring {
    reply(env, |_| {
        let mut slot = AGENT.lock().map_err(|_| "native_unavailable")?;
        if let Some(agent) = slot.take() {
            agent.stop();
        }
        Ok(json!({"phase":"disconnected"}))
    })
}

/// The isolated worker receives descriptors, never paths into application storage.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeDocumentBridge_extract<'local>(
    env: JNIEnv<'local>,
    _object: JObject<'local>,
    input: jni::sys::jint,
    extension: JString<'local>,
    mode: JString<'local>,
    output: jni::sys::jint,
    recognizer: JObject<'local>,
) -> jstring {
    reply(env, |env| {
        use std::{io::Write, os::fd::FromRawFd};
        let extension = string(env, &extension, 16)?;
        if !extension.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("unsupported_format");
        }
        let mode: pab_document::OcrMode = serde_json::from_value(json!(string(env, &mode, 8)?))
            .map_err(|_| "invalid_ocr_mode")?;
        if input < 0 || output < 0 || input == output {
            return Err("invalid_document_descriptor");
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "document_worker_unavailable")?;
        // SAFETY: Only the duplicated descriptor is owned by Rust. No path reopen
        // is permitted: the isolated UID cannot access application-private paths.
        let source = unsafe { libc::dup(input) };
        if source < 0 {
            return Err("invalid_document_descriptor");
        }
        let source = unsafe { std::fs::File::from_raw_fd(source) };
        struct PlatformOcr<'a, 'local> {
            env: &'a mut JNIEnv<'local>,
            callback: JObject<'local>,
        }
        impl pab_document::PageOcr for PlatformOcr<'_, '_> {
            fn recognize(
                &mut self,
                page: u32,
            ) -> Result<(String, f32), pab_document::DocumentError> {
                let encoded: Result<String, jni::errors::Error> =
                    self.env.with_local_frame(16, |env| {
                        let value = env
                            .call_method(
                                &self.callback,
                                "recognize",
                                "(I)Ljava/lang/String;",
                                &[jni::objects::JValue::Int(page as i32)],
                            )?
                            .l()?;
                        string(env, &JString::from(value), 1024 * 1024 + 4096)
                            .map_err(|_| jni::errors::Error::NullPtr("OCR result"))
                    });
                let value = match encoded {
                    Ok(value) => value,
                    Err(_) => {
                        let _ = self.env.exception_clear();
                        return Err(pab_document::DocumentError::Extraction(
                            "platform_ocr_failed".into(),
                        ));
                    }
                };
                let value = serde_json::from_str::<Value>(&value).ok().ok_or_else(|| {
                    pab_document::DocumentError::Extraction("platform_ocr_failed".into())
                })?;
                let text = value["text"].as_str().ok_or_else(|| {
                    pab_document::DocumentError::Extraction("platform_ocr_failed".into())
                })?;
                let confidence = value["confidence"].as_f64().ok_or_else(|| {
                    pab_document::DocumentError::Extraction("platform_ocr_failed".into())
                })?;
                Ok((text.to_owned(), confidence as f32))
            }
        }
        let mut recognizer = PlatformOcr {
            env,
            callback: recognizer,
        };
        let document = runtime
            .block_on(pab_document::extract_file_with_ocr(
                source,
                &extension,
                mode,
                &mut recognizer,
            ))
            .map_err(|error| match error {
                pab_document::DocumentError::PasswordRequired => "password_required",
                pab_document::DocumentError::OcrUnavailable => "ocr_unavailable",
                pab_document::DocumentError::NoText => "no_extractable_text",
                pab_document::DocumentError::ResourceLimit(_) => "document_resource_limit",
                pab_document::DocumentError::UnsupportedFormat(_) => "unsupported_format",
                pab_document::DocumentError::Extraction(ref reason)
                    if reason == "platform_ocr_failed" =>
                {
                    "ocr_recognition_failed"
                }
                _ => "document_parse_failed",
            })?;
        let bytes = serde_json::to_vec(&document).map_err(|_| "document_encode_failed")?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err("document_result_limit");
        }
        // SAFETY: dup creates a distinct owned descriptor; the Java caller retains its own.
        let fd = unsafe { libc::dup(output) };
        if fd < 0 {
            return Err("document_output_unavailable");
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "document_output_failed")?;
        Ok(json!({"ready":true}))
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeDocumentBridge_recognize(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
    gray: JByteArray<'_>,
    width: jni::sys::jint,
    height: jni::sys::jint,
    model: JByteArray<'_>,
) -> jstring {
    reply(env, |env| {
        if env
            .get_array_length(&gray)
            .map_err(|_| "invalid_ocr_input")?
            > 2048 * 2048
            || env
                .get_array_length(&model)
                .map_err(|_| "invalid_ocr_model")?
                != 2_469_156
        {
            return Err("invalid_ocr_input_or_model");
        }
        let gray = env
            .convert_byte_array(&gray)
            .map_err(|_| "invalid_ocr_input")?;
        let model = env
            .convert_byte_array(&model)
            .map_err(|_| "invalid_ocr_model")?;
        let (text, confidence) = crate::ocr::recognize(&gray, width, height, &model)?;
        Ok(json!({"text":text,"confidence":confidence}))
    })
}

/// Limits belong to the isolated process and cannot change the connection process.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_pixels_agent_transport_NativeDocumentBridge_limitResources(
    env: JNIEnv<'_>,
    _object: JObject<'_>,
) -> jstring {
    reply(env, |_env| {
        for (resource, ceiling) in [
            (libc::RLIMIT_FSIZE, 16 * 1024 * 1024),
            (libc::RLIMIT_CPU, 25),
        ] {
            let limit = libc::rlimit {
                rlim_cur: ceiling as libc::rlim_t,
                rlim_max: ceiling as libc::rlim_t,
            };
            // SAFETY: a valid rlimit pointer lowers only this disposable process's limits.
            if unsafe { libc::setrlimit(resource, &limit) } != 0 {
                return Err("document_limits_unavailable");
            }
        }
        Ok(json!({"limited":true}))
    })
}
