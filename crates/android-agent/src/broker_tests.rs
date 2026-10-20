use super::*;

#[tokio::test]
async fn competing_controllers_cannot_cancel_or_replay_each_others_input() {
    let directory = tempfile::tempdir().unwrap();
    let broker = PlatformBroker::open(&directory.path().join("ops.sqlite"), [12; 32])
        .await
        .unwrap();
    let first_scope = Arc::new(Mutex::new(ReferenceScope::default()));
    let second_scope = Arc::new(Mutex::new(ReferenceScope::default()));
    let id = RequestId::new();
    let first = MobileCommand::Navigate {
        destination: MobileNavigation::Home,
    };
    let second = MobileCommand::Navigate {
        destination: MobileNavigation::Back,
    };
    let (first_reply, second_reply) = tokio::join!(
        broker.request(
            "first-controller",
            first_scope.clone(),
            MobileRequest::Execute {
                request_id: id,
                command: first,
            },
        ),
        broker.request(
            "second-controller",
            second_scope.clone(),
            MobileRequest::Execute {
                request_id: id,
                command: second.clone(),
            },
        ),
    );
    assert_eq!(first_reply.state, MobileState::Running);
    assert_eq!(second_reply.state, MobileState::Running);
    let cancelled = broker
        .request(
            "first-controller",
            first_scope.clone(),
            MobileRequest::Cancel { request_id: id },
        )
        .await;
    assert_eq!(cancelled.state, MobileState::Cancelled);
    assert!(!cancelled.action_dispatched);
    let job = broker.poll().await.unwrap();
    assert_eq!(job["command"]["destination"], "back");
    assert_eq!(broker.poll().await.unwrap(), Value::Null);
    broker
        .complete(
            job["ticket"].as_str().unwrap().parse().unwrap(),
            json!({"acknowledged":true}),
            vec![],
        )
        .await
        .unwrap();
    for _ in 0..20 {
        let duplicate = broker
            .request(
                "second-controller",
                second_scope.clone(),
                MobileRequest::Execute {
                    request_id: id,
                    command: second.clone(),
                },
            )
            .await;
        assert_eq!(duplicate.state, MobileState::Completed);
        assert!(duplicate.action_dispatched && duplicate.platform_acknowledged);
    }
    assert_eq!(broker.poll().await.unwrap(), Value::Null);
    assert_eq!(
        broker
            .request(
                "first-controller",
                first_scope,
                MobileRequest::Get { request_id: id },
            )
            .await
            .state,
        MobileState::Cancelled
    );
}

#[tokio::test]
async fn capacity_maintenance_preserves_old_ids_and_accepts_new_work() {
    let directory = tempfile::tempdir().unwrap();
    let broker = PlatformBroker::open(&directory.path().join("ops.sqlite"), [11; 32])
        .await
        .unwrap();
    let scope = Arc::new(Mutex::new(ReferenceScope::default()));
    let command = MobileCommand::Navigate {
        destination: MobileNavigation::Home,
    };
    let old = RequestId::new();
    broker
        .request(
            "caller",
            scope.clone(),
            MobileRequest::Execute {
                request_id: old,
                command: command.clone(),
            },
        )
        .await;
    let ticket = broker.poll().await.unwrap()["ticket"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    broker
        .complete(ticket, json!({"acknowledged":true}), vec![])
        .await
        .unwrap();
    let mut tx = broker.pool.begin().await.unwrap();
    for _ in 0..9999 {
        let id = RequestId::new();
        let mut reply = MobileReply::pending(id);
        reply.state = MobileState::Completed;
        sqlx::query(
            "INSERT INTO mobile_operations(owner,id,digest,reply,claimed) VALUES(?,?,?,?,1)",
        )
        .bind("seed")
        .bind(id.to_string())
        .bind("seed-digest")
        .bind(serde_json::to_string(&reply).unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let fresh = RequestId::new();
    assert_eq!(
        broker
            .request(
                "caller",
                scope.clone(),
                MobileRequest::Execute {
                    request_id: fresh,
                    command: command.clone()
                }
            )
            .await
            .state,
        MobileState::Running
    );
    let archived: i64 = sqlx::query_scalar("SELECT count(*) FROM mobile_operation_archive")
        .fetch_one(&broker.pool)
        .await
        .unwrap();
    assert_eq!(archived, 1000);
    // Discard transient caches too: the archive alone must retain deduplication facts.
    broker.results.lock().await.clear();
    for _ in 0..20 {
        let previous = broker
            .request(
                "caller",
                scope.clone(),
                MobileRequest::Execute {
                    request_id: old,
                    command: command.clone(),
                },
            )
            .await;
        assert_eq!(previous.state, MobileState::Completed);
        assert!(previous.action_dispatched && previous.platform_acknowledged);
    }
    assert_eq!(
        broker
            .request(
                "caller",
                scope.clone(),
                MobileRequest::Execute {
                    request_id: old,
                    command: MobileCommand::Observe {}
                }
            )
            .await
            .error
            .as_deref(),
        Some("request_conflict")
    );
    assert_eq!(
        broker
            .request("stranger", scope, MobileRequest::Get { request_id: old })
            .await
            .error
            .as_deref(),
        Some("not_found")
    );
    assert!(!broker.poll().await.unwrap().is_null());
    assert_eq!(broker.poll().await.unwrap(), Value::Null);
}

#[tokio::test]
async fn interrupted_archive_transaction_does_not_forget_an_accepted_action() {
    let directory = tempfile::tempdir().unwrap();
    let broker = PlatformBroker::open(&directory.path().join("ops.sqlite"), [12; 32])
        .await
        .unwrap();
    let scope = Arc::new(Mutex::new(ReferenceScope::default()));
    let id = RequestId::new();
    let command = MobileCommand::Navigate {
        destination: MobileNavigation::Back,
    };
    broker
        .request(
            "caller",
            scope.clone(),
            MobileRequest::Execute {
                request_id: id,
                command: command.clone(),
            },
        )
        .await;
    let ticket = broker.poll().await.unwrap()["ticket"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    broker
        .complete(ticket, json!({"acknowledged":true}), vec![])
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_archive BEFORE DELETE ON mobile_operations BEGIN SELECT RAISE(ABORT,'injected'); END").execute(&broker.pool).await.unwrap();
    assert!(broker.archive_completed().await.is_err());
    let archived: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mobile_operation_archive")
        .fetch_one(&broker.pool)
        .await
        .unwrap();
    assert_eq!(archived, 0);
    sqlx::query("DROP TRIGGER reject_archive")
        .execute(&broker.pool)
        .await
        .unwrap();
    assert_eq!(broker.archive_completed().await.unwrap(), 1);
    broker.results.lock().await.clear();
    assert_eq!(
        broker
            .request(
                "caller",
                scope,
                MobileRequest::Execute {
                    request_id: id,
                    command
                }
            )
            .await
            .state,
        MobileState::Completed
    );
    assert_eq!(broker.poll().await.unwrap(), Value::Null);
}

#[tokio::test]
async fn disconnected_scope_cannot_dispatch_queued_actions() {
    let directory = tempfile::tempdir().unwrap();
    let broker = PlatformBroker::open(&directory.path().join("ops.sqlite"), [9; 32])
        .await
        .unwrap();
    let refs = ReferenceScope::default();
    let lease = refs.lease();
    let scope = Arc::new(Mutex::new(refs));
    let id = RequestId::new();
    broker
        .request(
            "caller",
            scope.clone(),
            MobileRequest::Execute {
                request_id: id,
                command: MobileCommand::Navigate {
                    destination: MobileNavigation::Home,
                },
            },
        )
        .await;
    drop(lease);
    assert_eq!(broker.poll().await.unwrap(), Value::Null);
    let cancelled = broker
        .request("caller", scope, MobileRequest::Get { request_id: id })
        .await;
    assert_eq!(cancelled.state, MobileState::Cancelled);
    assert!(!cancelled.action_dispatched);
}

#[tokio::test]
async fn malformed_callback_is_terminal_and_never_replayed() {
    let directory = tempfile::tempdir().unwrap();
    let broker = PlatformBroker::open(&directory.path().join("ops.sqlite"), [10; 32])
        .await
        .unwrap();
    let scope = Arc::new(Mutex::new(ReferenceScope::default()));
    let id = RequestId::new();
    let command = MobileCommand::Screenshot {};
    broker
        .request(
            "caller",
            scope.clone(),
            MobileRequest::Execute {
                request_id: id,
                command: command.clone(),
            },
        )
        .await;
    let ticket = broker.poll().await.unwrap()["ticket"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    broker
        .complete(
            ticket,
            json!({"width":u64::MAX,"height":u64::MAX}),
            vec![0xff, 0xd8],
        )
        .await
        .unwrap();
    let rejected = broker
        .request(
            "caller",
            scope,
            MobileRequest::Execute {
                request_id: id,
                command,
            },
        )
        .await;
    assert_eq!(rejected.state, MobileState::Failed);
    assert_eq!(rejected.error.as_deref(), Some("invalid_image"));
    assert_eq!(broker.poll().await.unwrap(), Value::Null);
}
#[tokio::test]
async fn cancellation_and_restart_never_replay_dispatched_input() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.sqlite");
    let broker = PlatformBroker::open(&path, [7; 32]).await.unwrap();
    let scope = Arc::new(Mutex::new(ReferenceScope::default()));
    let id = RequestId::new();
    let command = MobileCommand::Navigate {
        destination: MobileNavigation::Back,
    };
    assert_eq!(
        broker
            .request(
                "caller",
                scope.clone(),
                MobileRequest::Execute {
                    request_id: id,
                    command: command.clone()
                }
            )
            .await
            .state,
        MobileState::Running
    );
    assert_eq!(
        broker
            .request(
                "other",
                scope.clone(),
                MobileRequest::Get { request_id: id }
            )
            .await
            .error
            .as_deref(),
        Some("not_found")
    );
    assert_eq!(
        broker
            .request(
                "caller",
                scope.clone(),
                MobileRequest::Cancel { request_id: id }
            )
            .await
            .state,
        MobileState::Cancelled
    );
    assert_eq!(broker.poll().await.unwrap(), Value::Null);
    let second = RequestId::new();
    broker
        .request(
            "caller",
            scope.clone(),
            MobileRequest::Execute {
                request_id: second,
                command: command.clone(),
            },
        )
        .await;
    assert!(!broker.poll().await.unwrap().is_null());
    broker.pool.close().await;
    let restored = PlatformBroker::open(&path, [7; 32]).await.unwrap();
    let result = restored
        .request(
            "caller",
            scope.clone(),
            MobileRequest::Execute {
                request_id: second,
                command,
            },
        )
        .await;
    assert_eq!(result.state, MobileState::Unconfirmed);
    assert!(result.action_dispatched);
    assert_eq!(restored.poll().await.unwrap(), Value::Null);
    assert_eq!(
        restored
            .request(
                "caller",
                scope,
                MobileRequest::Execute {
                    request_id: second,
                    command: MobileCommand::Observe {}
                }
            )
            .await
            .error
            .as_deref(),
        Some("request_conflict")
    );
}

#[tokio::test]
async fn references_are_connection_bound_and_screen_payloads_are_not_persisted() {
    let directory = tempfile::tempdir().unwrap();
    let broker = PlatformBroker::open(&directory.path().join("ops.sqlite"), [8; 32])
        .await
        .unwrap();
    let scope = Arc::new(Mutex::new(ReferenceScope::default()));
    let id = RequestId::new();
    broker
        .request(
            "caller",
            scope.clone(),
            MobileRequest::Execute {
                request_id: id,
                command: MobileCommand::Observe {},
            },
        )
        .await;
    let ticket = broker.poll().await.unwrap()["ticket"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let native = Uuid::new_v4().to_string();
    let snapshot = json!({"observation":{"observation_ref":Uuid::new_v4(),"revision":1,"width":720,"height":1280,"rotation":0,"truncated":false,
            "elements":[{"element_ref":native,"role":"button","name":"private screen content","bounds":{"left":0,"top":0,"right":10,"bottom":10},"enabled":true,"clickable":true,"editable":false,"protected":false}]}});
    broker.complete(ticket, snapshot, vec![]).await.unwrap();
    let reply = broker
        .request(
            "caller",
            scope.clone(),
            MobileRequest::Get { request_id: id },
        )
        .await;
    let issued = reply.observation.unwrap().elements[0].element_ref.clone();
    assert_ne!(issued, native);
    let stranger = Arc::new(Mutex::new(ReferenceScope::default()));
    assert_eq!(
        broker
            .request(
                "caller",
                stranger,
                MobileRequest::Execute {
                    request_id: RequestId::new(),
                    command: MobileCommand::Click {
                        element_ref: issued.clone()
                    }
                }
            )
            .await
            .error
            .as_deref(),
        Some("target_stale")
    );
    let click = RequestId::new();
    let command = MobileCommand::Click {
        element_ref: issued,
    };
    broker
        .request(
            "caller",
            scope.clone(),
            MobileRequest::Execute {
                request_id: click,
                command: command.clone(),
            },
        )
        .await;
    let job = broker.poll().await.unwrap();
    assert_eq!(job["command"]["reference"], native);
    broker
        .complete(
            job["ticket"].as_str().unwrap().parse().unwrap(),
            json!({"acknowledged":true}),
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(
        broker
            .request(
                "caller",
                scope,
                MobileRequest::Execute {
                    request_id: click,
                    command
                }
            )
            .await
            .state,
        MobileState::Completed
    );
    assert_eq!(broker.poll().await.unwrap(), Value::Null);
    let saved: Vec<String> = sqlx::query_scalar("SELECT reply FROM mobile_operations")
        .fetch_all(&broker.pool)
        .await
        .unwrap();
    assert!(
        saved
            .iter()
            .all(|text| !text.contains("private screen content") && !text.contains(&native))
    );
}
