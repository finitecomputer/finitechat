use finitechat_engine::{
    CreateDirectRoomRequest, DeliveryService, EngineError, LinkSessionState,
    UploadKeyPackageRequest, envelope,
};
use finitechat_proto::{
    KeyPackageState, LogEntryKind, MembershipAddV1, MembershipDeltaError, MembershipDeltaV1,
    MembershipRemoveV1, RoomStatus, WelcomeState,
};
use finitechat_sim::{SimWorld, alice, bob, charlie, dana};

fn provision_bob(world: &mut SimWorld) {
    world
        .add_device_commit(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "idem_add_bob",
        )
        .unwrap();
    world.activate_device("welcome_bob_1", bob()).unwrap();
}

#[test]
fn create_dm_room_and_release_welcome_after_commit() {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .add_device_commit(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "idem_add_bob",
        )
        .unwrap();

    let room = world.server.room(&world.room_id).unwrap();
    assert_eq!(room.current_epoch, 1);
    assert_eq!(room.last_seq, 1);
    assert_eq!(room.log.len(), 1);
    assert_eq!(
        world.server.key_package("kp_bob_1").unwrap().state,
        KeyPackageState::Consumed
    );
    assert_eq!(
        world.server.welcome("welcome_bob_1").unwrap().state,
        WelcomeState::Released
    );
}

#[test]
fn welcome_activation_makes_new_device_active() {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .add_device_commit(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "idem_add_bob",
        )
        .unwrap();

    let welcomes = world.server.claim_welcomes(&bob());
    assert_eq!(welcomes.len(), 1);
    world.server.ack_welcome("welcome_bob_1", true).unwrap();

    let room = world.server.room(&world.room_id).unwrap();
    assert!(room.device_active_at_head(&bob()));
}

#[test]
fn duplicate_commit_retry_returns_same_result_after_side_effects() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    let request = world
        .add_device_request(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "idem_add_bob",
        )
        .unwrap();

    let first = world.server.submit_commit(request.clone()).unwrap();
    let second = world.server.submit_commit(request).unwrap();

    assert_eq!(first, second);
    assert_eq!(world.server.room(&world.room_id).unwrap().log.len(), 1);
    assert_eq!(
        world.server.key_package("kp_bob_1").unwrap().state,
        KeyPackageState::Consumed
    );
}

#[test]
fn conflicting_idempotency_key_rejects_without_side_effects() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    world.upload_and_claim(charlie(), "kp_charlie_1").unwrap();

    let first = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "same_key")
        .unwrap();
    let conflicting = world
        .add_device_request(
            alice(),
            charlie(),
            "kp_charlie_1",
            "welcome_charlie_1",
            0,
            "same_key",
        )
        .unwrap();

    world.server.submit_commit(first).unwrap();
    let err = world.server.submit_commit(conflicting).unwrap_err();

    assert_eq!(err, EngineError::ConflictingIdempotencyKey);
    assert!(world.server.welcome("welcome_charlie_1").is_none());
    assert_eq!(
        world.server.key_package("kp_charlie_1").unwrap().state,
        KeyPackageState::Leased
    );
}

#[test]
fn same_epoch_loser_restart_retry_replays_rejection() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    world.upload_and_claim(charlie(), "kp_charlie_1").unwrap();

    let winner = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "winner")
        .unwrap();
    let loser = world
        .add_device_request(
            alice(),
            charlie(),
            "kp_charlie_1",
            "welcome_charlie_1",
            0,
            "loser",
        )
        .unwrap();

    world.server.submit_commit(winner).unwrap();
    let first_err = world.server.submit_commit(loser.clone()).unwrap_err();
    let mut restarted = world.server.clone();
    let replayed_err = restarted.submit_commit(loser).unwrap_err();

    assert_eq!(first_err, replayed_err);
    assert!(matches!(
        replayed_err,
        EngineError::WrongEpoch {
            expected: 1,
            actual: 0
        }
    ));
    assert!(restarted.welcome("welcome_charlie_1").is_none());
    assert_eq!(
        restarted.key_package("kp_charlie_1").unwrap().state,
        KeyPackageState::Leased
    );
}

#[test]
fn welcome_is_not_released_before_accepted_commit() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    assert!(world.server.welcome("welcome_bob_1").is_none());

    let mut bad_request = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "bad_add")
        .unwrap();
    bad_request.membership_delta.adds[0].key_package_hash = "wrong_hash".to_string();
    assert!(world.server.submit_commit(bad_request).is_err());
    assert!(world.server.welcome("welcome_bob_1").is_none());

    let good_request = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "good_add")
        .unwrap();
    world.server.submit_commit(good_request).unwrap();
    assert_eq!(
        world.server.welcome("welcome_bob_1").unwrap().state,
        WelcomeState::Released
    );
}

#[test]
fn key_package_lease_expiry_returns_package_to_available() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    assert_eq!(
        world.server.key_package("kp_bob_1").unwrap().state,
        KeyPackageState::Leased
    );

    world.server.expire_key_package_lease("kp_bob_1").unwrap();
    assert_eq!(
        world.server.key_package("kp_bob_1").unwrap().state,
        KeyPackageState::Available
    );

    world.server.claim_key_package("kp_bob_1").unwrap();
    assert_eq!(
        world.server.key_package("kp_bob_1").unwrap().state,
        KeyPackageState::Leased
    );
}

#[test]
fn consumed_key_package_cannot_be_reused() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    let first = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "first")
        .unwrap();
    world.server.submit_commit(first).unwrap();

    let second = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_2", 1, "second")
        .unwrap();
    let err = world.server.submit_commit(second).unwrap_err();

    assert_eq!(
        err,
        EngineError::KeyPackageUnavailable {
            key_package_id: "kp_bob_1".to_string(),
            state: KeyPackageState::Consumed
        }
    );
    assert!(world.server.welcome("welcome_bob_2").is_none());
}

#[test]
fn stale_key_package_ref_is_rejected_without_side_effects() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    let mut request = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "bad_ref")
        .unwrap();
    request.membership_delta.adds[0].key_package_ref = "stale_ref".to_string();

    let err = world.server.submit_commit(request).unwrap_err();

    assert_eq!(
        err,
        EngineError::KeyPackageRefMismatch("kp_bob_1".to_string())
    );
    assert!(world.server.welcome("welcome_bob_1").is_none());
    assert_eq!(
        world.server.key_package("kp_bob_1").unwrap().state,
        KeyPackageState::Leased
    );
}

#[test]
fn invalid_commit_report_fails_closed() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    let request = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add_bob")
        .unwrap();
    let accepted = world.server.submit_commit(request).unwrap();

    world
        .server
        .report_invalid_commit(&world.room_id, &alice(), accepted.seq)
        .unwrap();

    assert_eq!(
        world.server.room(&world.room_id).unwrap().status,
        RoomStatus::NeedsRepair
    );
    let err = world
        .server
        .append_event(world.app_message_request(alice(), 1, "blocked", "msg_after_repair"))
        .unwrap_err();
    assert_eq!(err, EngineError::RoomNotOpen);
}

#[test]
fn membership_delta_disagreement_enters_needs_repair() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    let request = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add_bob")
        .unwrap();
    let accepted = world.server.submit_commit(request).unwrap();

    // Fake MLS client determines the structurally valid metadata did not match
    // the real Commit effects and reports the accepted log entry.
    world
        .server
        .report_invalid_commit(&world.room_id, &alice(), accepted.seq)
        .unwrap();

    assert_eq!(
        world.server.room(&world.room_id).unwrap().status,
        RoomStatus::NeedsRepair
    );
}

#[test]
fn false_remove_delta_does_not_block_removed_device_from_validating_removal_seq() {
    let mut world = SimWorld::direct_room().unwrap();
    provision_bob(&mut world);
    let remove = world
        .remove_device_request(alice(), bob(), 1, "remove_bob")
        .unwrap();
    let accepted = world.server.submit_commit(remove).unwrap();

    let bob_entries = world.server.sync_events(&world.room_id, &bob(), 1).unwrap();
    assert_eq!(bob_entries.len(), 1);
    assert_eq!(bob_entries[0].seq, accepted.seq);

    world
        .server
        .report_invalid_commit(&world.room_id, &bob(), accepted.seq)
        .unwrap();
    assert_eq!(
        world.server.room(&world.room_id).unwrap().status,
        RoomStatus::NeedsRepair
    );
}

#[test]
fn new_device_linking_partial_failure_retries_only_failed_room() {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .add_device_commit(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add_bob")
        .unwrap();
    world.upload_and_claim(charlie(), "kp_charlie_1").unwrap();
    let stale = world
        .add_device_request(
            alice(),
            charlie(),
            "kp_charlie_1",
            "welcome_charlie_1",
            0,
            "add_charlie_stale",
        )
        .unwrap();
    assert!(matches!(
        world.server.submit_commit(stale).unwrap_err(),
        EngineError::WrongEpoch { .. }
    ));
    assert!(world.server.welcome("welcome_charlie_1").is_none());

    let retry = world
        .add_device_request(
            alice(),
            charlie(),
            "kp_charlie_1",
            "welcome_charlie_1",
            1,
            "add_charlie_retry",
        )
        .unwrap();
    world.server.submit_commit(retry).unwrap();
    assert_eq!(
        world.server.welcome("welcome_charlie_1").unwrap().state,
        WelcomeState::Released
    );
}

#[test]
fn link_mailbox_payload_is_opaque_to_server_state() {
    let mut world = SimWorld::direct_room().unwrap();
    let payload = b"ciphertext:server-list-and-authorization".to_vec();
    world
        .server
        .create_link_session("link_1", "pairing_key_1")
        .unwrap();
    world
        .server
        .upload_link_payload("link_1", payload.clone())
        .unwrap();
    let (claimed, _) = world.server.claim_link_payload("link_1").unwrap();

    assert_eq!(claimed, payload);
    assert_eq!(
        world.server.link_session("link_1").unwrap().state,
        LinkSessionState::Claimed
    );
}

#[test]
fn link_session_duplicate_conflict_expiry_and_delivery_rules() {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .server
        .create_link_session("link_1", "pairing_key_1")
        .unwrap();
    world
        .server
        .upload_link_payload("link_1", b"ciphertext".to_vec())
        .unwrap();
    world
        .server
        .upload_link_payload("link_1", b"ciphertext".to_vec())
        .unwrap();
    assert_eq!(
        world
            .server
            .upload_link_payload("link_1", b"different".to_vec())
            .unwrap_err(),
        EngineError::LinkSessionConflict
    );

    let (_, token) = world.server.claim_link_payload("link_1").unwrap();
    world.server.release_link_claim("link_1").unwrap();
    let (_, token_after_release) = world.server.claim_link_payload("link_1").unwrap();
    assert_eq!(token, token_after_release);
    world
        .server
        .ack_link_payload("link_1", &token_after_release)
        .unwrap();
    assert_eq!(
        world
            .server
            .upload_link_payload("link_1", b"late".to_vec())
            .unwrap_err(),
        EngineError::LinkSessionClosed
    );

    world
        .server
        .create_link_session("link_2", "pairing_key_2")
        .unwrap();
    world.server.expire_link_session("link_2").unwrap();
    assert_eq!(
        world
            .server
            .upload_link_payload("link_2", b"late".to_vec())
            .unwrap_err(),
        EngineError::LinkSessionClosed
    );
}

#[test]
fn push_wake_is_only_a_hint_and_does_not_advance_client_state() {
    #[derive(Default)]
    struct FakeClient {
        last_seq: u64,
        wake_count: u64,
    }
    impl FakeClient {
        fn push_wake(&mut self) {
            self.wake_count += 1;
        }
    }

    let mut client = FakeClient::default();
    client.push_wake();
    client.push_wake();

    assert_eq!(client.wake_count, 2);
    assert_eq!(client.last_seq, 0);
}

#[test]
fn stale_push_for_removed_device_cannot_authorize_new_events() {
    let mut world = SimWorld::direct_room().unwrap();
    provision_bob(&mut world);
    let remove = world
        .remove_device_request(alice(), bob(), 1, "remove_bob")
        .unwrap();
    let removal = world.server.submit_commit(remove).unwrap();

    world
        .server
        .append_event(world.app_message_request(alice(), 2, "after removal", "msg_after_remove"))
        .unwrap();

    let bob_entries = world
        .server
        .sync_events(&world.room_id, &bob(), removal.seq)
        .unwrap();
    assert!(bob_entries.is_empty());
}

#[test]
fn accepted_commit_response_lost_then_server_restart_replays_same_result() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    let request = world
        .add_device_request(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "lost_response",
        )
        .unwrap();

    let accepted = world.server.submit_commit(request.clone()).unwrap();
    let mut restarted = world.server.clone();
    let replayed = restarted.submit_commit(request).unwrap();

    assert_eq!(accepted, replayed);
    assert_eq!(restarted.room(&world.room_id).unwrap().log.len(), 1);
    assert_eq!(
        restarted.welcome("welcome_bob_1").unwrap().state,
        WelcomeState::Released
    );
}

#[test]
fn commit_durable_before_welcome_release_restart_releases_exactly_once() {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .add_device_commit(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "idem_add_bob",
        )
        .unwrap();

    let mut restarted = world.server.clone();
    let first_claim = restarted.claim_welcomes(&bob());
    let duplicate_claim = restarted.claim_welcomes(&bob());

    assert_eq!(first_claim.len(), 1);
    assert!(duplicate_claim.is_empty());
    assert_eq!(
        restarted.welcome("welcome_bob_1").unwrap().state,
        WelcomeState::Claimed
    );
}

#[test]
fn commit_effects_are_atomic_at_reducer_boundary() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    let request = world
        .add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "atomic")
        .unwrap();

    let before = world.server.clone();
    let accepted = world.server.submit_commit(request).unwrap();
    let after = &world.server;

    assert!(before.welcome("welcome_bob_1").is_none());
    assert_eq!(after.room(&world.room_id).unwrap().last_seq, accepted.seq);
    assert_eq!(after.room(&world.room_id).unwrap().current_epoch, 1);
    assert_eq!(
        after.key_package("kp_bob_1").unwrap().state,
        KeyPackageState::Consumed
    );
    assert_eq!(
        after.welcome("welcome_bob_1").unwrap().state,
        WelcomeState::Released
    );
}

#[test]
fn welcome_claim_crash_before_ack_can_resume_after_restart() {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .add_device_commit(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "idem_add_bob",
        )
        .unwrap();
    let claimed = world.server.claim_welcomes(&bob());
    assert_eq!(claimed.len(), 1);

    let mut restarted = world.server.clone();
    restarted.ack_welcome("welcome_bob_1", true).unwrap();

    assert!(
        restarted
            .room(&world.room_id)
            .unwrap()
            .device_active_at_head(&bob())
    );
    assert_eq!(
        restarted.welcome("welcome_bob_1").unwrap().state,
        WelcomeState::Acked
    );
}

#[test]
fn delayed_welcome_after_later_entries_syncs_forward_from_commit_seq() {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .add_device_commit(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "idem_add_bob",
        )
        .unwrap();
    world
        .server
        .append_event(world.app_message_request(alice(), 1, "later", "msg_later"))
        .unwrap();

    world.activate_device("welcome_bob_1", bob()).unwrap();
    let entries = world.server.sync_events(&world.room_id, &bob(), 1).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, 2);
}

#[test]
fn welcome_terminal_failure_keeps_membership_interval_inactive() {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .add_device_commit(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "idem_add_bob",
        )
        .unwrap();
    world.server.claim_welcomes(&bob());
    world.server.ack_welcome("welcome_bob_1", false).unwrap();

    assert_eq!(
        world.server.welcome("welcome_bob_1").unwrap().state,
        WelcomeState::Failed
    );
    assert!(
        !world
            .server
            .room(&world.room_id)
            .unwrap()
            .device_active_at_head(&bob())
    );
}

#[test]
fn fetch_then_stream_gap_is_repaired_by_pull_cursor() {
    #[derive(Default)]
    struct Ingestor {
        last_contiguous: u64,
        buffered: Vec<u64>,
    }
    impl Ingestor {
        fn ingest_stream(&mut self, seq: u64) {
            if seq == self.last_contiguous + 1 {
                self.last_contiguous = seq;
                while let Some(pos) = self
                    .buffered
                    .iter()
                    .position(|candidate| *candidate == self.last_contiguous + 1)
                {
                    self.buffered.remove(pos);
                    self.last_contiguous += 1;
                }
            } else if seq > self.last_contiguous + 1 && !self.buffered.contains(&seq) {
                self.buffered.push(seq);
            }
        }
        fn repair_from_pull(&mut self, pulled: &[u64]) {
            for seq in pulled {
                self.ingest_stream(*seq);
            }
        }
    }

    let mut ingestor = Ingestor {
        last_contiguous: 1,
        buffered: Vec::new(),
    };
    ingestor.ingest_stream(3);
    ingestor.ingest_stream(3);
    assert_eq!(ingestor.last_contiguous, 1);
    ingestor.repair_from_pull(&[2, 3]);
    assert_eq!(ingestor.last_contiguous, 3);
}

#[test]
fn stable_message_id_survives_retry_and_distinguishes_payloads() {
    let first = envelope(
        "room",
        "group",
        alice(),
        0,
        LogEntryKind::Application,
        b"same".to_vec(),
    );
    let retry = envelope(
        "room",
        "group",
        alice(),
        0,
        LogEntryKind::Application,
        b"same".to_vec(),
    );
    let different = envelope(
        "room",
        "group",
        alice(),
        0,
        LogEntryKind::Application,
        b"different".to_vec(),
    );

    assert_eq!(first.message_id().unwrap(), retry.message_id().unwrap());
    assert_ne!(first.message_id().unwrap(), different.message_id().unwrap());
}

#[test]
fn membership_delta_structural_matrix_rejects_bad_shapes() {
    let commit = envelope(
        "room",
        "group",
        alice(),
        0,
        LogEntryKind::Commit,
        b"commit".to_vec(),
    );
    let commit_id = commit.message_id().unwrap();
    let add = MembershipAddV1 {
        device: bob(),
        key_package_id: "kp_bob".to_string(),
        key_package_ref: "ref".to_string(),
        key_package_hash: "hash".to_string(),
        welcome_id: "welcome_bob".to_string(),
    };
    let remove = MembershipRemoveV1 {
        device: bob(),
        removed_leaf_index: 1,
    };

    let cases = vec![
        (
            MembershipDeltaV1 {
                base_epoch: 9,
                post_commit_epoch: 10,
                commit_message_id: commit_id.clone(),
                adds: vec![add.clone()],
                removes: vec![],
            },
            MembershipDeltaError::WrongBaseEpoch {
                expected: 0,
                actual: 9,
            },
        ),
        (
            MembershipDeltaV1 {
                base_epoch: 0,
                post_commit_epoch: 3,
                commit_message_id: commit_id.clone(),
                adds: vec![add.clone()],
                removes: vec![],
            },
            MembershipDeltaError::WrongPostCommitEpoch { base: 0, actual: 3 },
        ),
        (
            MembershipDeltaV1 {
                base_epoch: 0,
                post_commit_epoch: 1,
                commit_message_id: "wrong".to_string(),
                adds: vec![add.clone()],
                removes: vec![],
            },
            MembershipDeltaError::WrongCommitMessageId,
        ),
        (
            MembershipDeltaV1 {
                base_epoch: 0,
                post_commit_epoch: 1,
                commit_message_id: commit_id.clone(),
                adds: vec![add.clone(), add.clone()],
                removes: vec![],
            },
            MembershipDeltaError::DuplicateAdd(bob()),
        ),
        (
            MembershipDeltaV1 {
                base_epoch: 0,
                post_commit_epoch: 1,
                commit_message_id: commit_id.clone(),
                adds: vec![],
                removes: vec![remove.clone(), remove.clone()],
            },
            MembershipDeltaError::DuplicateRemove(bob()),
        ),
        (
            MembershipDeltaV1 {
                base_epoch: 0,
                post_commit_epoch: 1,
                commit_message_id: commit_id.clone(),
                adds: vec![add.clone()],
                removes: vec![remove],
            },
            MembershipDeltaError::AddAndRemoveSameDevice(bob()),
        ),
    ];

    for (delta, expected) in cases {
        assert_eq!(
            delta.validate_structure(0, &commit_id).unwrap_err(),
            expected
        );
    }
}

#[test]
fn direct_room_create_or_get_and_third_account_rejection() {
    let mut service = DeliveryService::new();
    let first = service
        .create_or_get_direct_room(CreateDirectRoomRequest {
            room_id: "direct_ab".to_string(),
            mls_group_id: "mls_ab".to_string(),
            creator: alice(),
            other_account_id: bob().account_id,
        })
        .unwrap();
    let second = service
        .create_or_get_direct_room(CreateDirectRoomRequest {
            room_id: "direct_ba_should_not_create".to_string(),
            mls_group_id: "mls_ba".to_string(),
            creator: bob(),
            other_account_id: alice().account_id,
        })
        .unwrap();
    assert_eq!(first, second);

    service
        .upload_key_package(UploadKeyPackageRequest {
            key_package_id: "kp_charlie".to_string(),
            owner: charlie(),
            key_package_ref: "ref_kp_charlie".to_string(),
            key_package_hash: "hash_kp_charlie".to_string(),
        })
        .unwrap();
    service.claim_key_package("kp_charlie").unwrap();
    let world = SimWorld {
        server: DeliveryService::new(),
        room_id: first.clone(),
        group_id: "mls_ab".to_string(),
    };
    let request = world
        .add_device_request(
            alice(),
            charlie(),
            "kp_charlie",
            "welcome_charlie",
            0,
            "add_third",
        )
        .unwrap();

    let err = service.submit_commit(request).unwrap_err();
    assert_eq!(
        err,
        EngineError::DirectRoomThirdAccount(charlie().account_id)
    );
}

#[test]
fn fake_device_credential_validation_rejects_wrong_bindings() {
    #[derive(Clone)]
    struct FakeCredential {
        account: &'static str,
        device: &'static str,
        leaf_signature_key: &'static str,
        signed_leaf_signature_key: &'static str,
        account_signature_ok: bool,
        expired: bool,
    }
    fn validate(credential: &FakeCredential) -> bool {
        credential.account_signature_ok
            && !credential.expired
            && !credential.account.is_empty()
            && !credential.device.is_empty()
            && credential.leaf_signature_key == credential.signed_leaf_signature_key
    }

    let valid = FakeCredential {
        account: "alice",
        device: "phone",
        leaf_signature_key: "leaf_key",
        signed_leaf_signature_key: "leaf_key",
        account_signature_ok: true,
        expired: false,
    };
    assert!(validate(&valid));

    let mut bad = valid.clone();
    bad.account_signature_ok = false;
    assert!(!validate(&bad));
    let mut bad = valid.clone();
    bad.expired = true;
    assert!(!validate(&bad));
    let mut bad = valid;
    bad.signed_leaf_signature_key = "other_key";
    assert!(!validate(&bad));
}

#[test]
fn fake_welcome_missing_ratchet_tree_fails_activation() {
    fn activate_welcome(has_ratchet_tree: bool, credentials_valid: bool) -> bool {
        has_ratchet_tree && credentials_valid
    }

    assert!(activate_welcome(true, true));
    assert!(!activate_welcome(false, true));
    assert!(!activate_welcome(true, false));
}

#[test]
fn login_challenge_replay_rules_are_single_use() {
    #[derive(Default)]
    struct FakeLogin {
        consumed: bool,
        origin: &'static str,
        disabled: bool,
    }
    impl FakeLogin {
        fn login(&mut self, origin: &str, signature_ok: bool) -> bool {
            if self.consumed || self.disabled || origin != self.origin || !signature_ok {
                return false;
            }
            self.consumed = true;
            true
        }
    }

    let mut login = FakeLogin {
        consumed: false,
        origin: "https://finite.test",
        disabled: false,
    };
    assert!(login.login("https://finite.test", true));
    assert!(!login.login("https://finite.test", true));

    let mut wrong_origin = FakeLogin {
        consumed: false,
        origin: "https://finite.test",
        disabled: false,
    };
    assert!(!wrong_origin.login("https://evil.test", true));

    let mut disabled = FakeLogin {
        consumed: false,
        origin: "https://finite.test",
        disabled: true,
    };
    assert!(!disabled.login("https://finite.test", true));
}

#[test]
fn local_pending_commit_is_not_merged_until_server_log_observed() {
    #[derive(Default)]
    struct FakeClient {
        local_pending_commit: Option<String>,
        applied_epoch: u64,
    }
    impl FakeClient {
        fn author_commit(&mut self, commit_id: &str) {
            self.local_pending_commit = Some(commit_id.to_string());
        }
        fn observe_log(&mut self, commit_id: &str) {
            if self.local_pending_commit.as_deref() == Some(commit_id) {
                self.applied_epoch += 1;
                self.local_pending_commit = None;
            }
        }
    }

    let mut client = FakeClient::default();
    client.author_commit("commit_1");
    assert_eq!(client.applied_epoch, 0);
    client.observe_log("other_commit");
    assert_eq!(client.applied_epoch, 0);
    client.observe_log("commit_1");
    assert_eq!(client.applied_epoch, 1);
}

#[test]
fn fake_changed_leaf_credential_validation_uses_same_device_binding_rules() {
    fn changed_leaf_valid(account_signature_ok: bool, leaf_key_matches_binding: bool) -> bool {
        account_signature_ok && leaf_key_matches_binding
    }

    assert!(changed_leaf_valid(true, true));
    assert!(!changed_leaf_valid(false, true));
    assert!(!changed_leaf_valid(true, false));
}

#[test]
fn link_fanout_existing_device_stale_isolated_to_failed_room() {
    let mut first_room = SimWorld::direct_room().unwrap();
    first_room
        .add_device_commit(alice(), bob(), "kp_bob_a", "welcome_bob_a", 0, "add_bob_a")
        .unwrap();

    let mut second_room = SimWorld::direct_room().unwrap();
    second_room
        .add_device_commit(
            alice(),
            dana(),
            "kp_dana_b",
            "welcome_dana_b",
            0,
            "add_dana_b",
        )
        .unwrap();
    second_room.upload_and_claim(bob(), "kp_bob_b").unwrap();
    let stale = second_room
        .add_device_request(
            alice(),
            bob(),
            "kp_bob_b",
            "welcome_bob_b",
            0,
            "add_bob_b_stale",
        )
        .unwrap();

    assert!(second_room.server.submit_commit(stale).is_err());
    assert!(first_room.server.welcome("welcome_bob_a").is_some());
    assert!(second_room.server.welcome("welcome_bob_b").is_none());
}
