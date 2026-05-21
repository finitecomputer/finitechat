use finitechat_engine::{
    AppendEventRequest, CreateDirectRoomRequest, CreateRoomRequest, DeliveryService, EngineError,
    LinkSessionState, ListAccountRoomsRequest, SubmitCommitRequest, UploadKeyPackageRequest,
    device, envelope,
};
use finitechat_proto::{
    DeviceRef, KeyPackageState, LogEntryKind, MAX_ACCOUNT_DEVICES_PER_ROOM,
    MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT, MAX_ENVELOPE_PAYLOAD_BYTES,
    MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE, MAX_SYNC_PAGE_ENTRIES, MembershipAddV1,
    MembershipDeltaError, MembershipDeltaV1, MembershipRemoveV1, ProtocolLimitError, RoomStatus,
    WelcomeState,
};
use finitechat_sim::{
    SimWorld, alice, bob, charlie, dana, fake_key_package_payload, staged_welcome,
};

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

fn upload_available_key_package(
    server: &mut DeliveryService,
    owner: DeviceRef,
    key_package_id: &str,
) {
    server
        .upload_key_package(UploadKeyPackageRequest {
            key_package_id: key_package_id.to_string(),
            owner,
            key_package_ref: format!("ref_{key_package_id}"),
            key_package_hash: format!("hash_{key_package_id}"),
            key_package_payload: fake_key_package_payload(key_package_id),
        })
        .unwrap();
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
fn key_package_claim_returns_opaque_payload() {
    let mut world = SimWorld::direct_room().unwrap();

    let claimed = world.upload_and_claim(bob(), "kp_bob_1").unwrap();

    assert_eq!(claimed.key_package_id, "kp_bob_1");
    assert_eq!(claimed.owner, bob());
    assert_eq!(claimed.key_package_ref, "ref_kp_bob_1");
    assert_eq!(claimed.key_package_hash, "hash_kp_bob_1");
    assert_eq!(
        claimed.key_package_payload,
        fake_key_package_payload("kp_bob_1")
    );
}

#[test]
fn account_key_package_claim_returns_one_available_package_per_device() {
    let mut server = DeliveryService::new();
    let bob_phone = device("bob_npub", "bob_phone");
    let bob_laptop = device("bob_npub", "bob_laptop");

    upload_available_key_package(&mut server, bob_phone.clone(), "kp_bob_phone_1");
    upload_available_key_package(&mut server, bob_phone.clone(), "kp_bob_phone_2");
    upload_available_key_package(&mut server, bob_laptop.clone(), "kp_bob_laptop_1");
    upload_available_key_package(&mut server, charlie(), "kp_charlie_1");

    let claimed = server.claim_key_packages_for_account("bob_npub").unwrap();

    assert_eq!(claimed.len(), 2);
    assert_eq!(claimed[0].owner, bob_laptop);
    assert_eq!(claimed[0].key_package_id, "kp_bob_laptop_1");
    assert_eq!(claimed[1].owner, bob_phone);
    assert_eq!(claimed[1].key_package_id, "kp_bob_phone_1");
    assert_eq!(
        server.key_package("kp_bob_laptop_1").unwrap().state,
        KeyPackageState::Leased
    );
    assert_eq!(
        server.key_package("kp_bob_phone_1").unwrap().state,
        KeyPackageState::Leased
    );
    assert_eq!(
        server.key_package("kp_bob_phone_2").unwrap().state,
        KeyPackageState::Available
    );
    assert_eq!(
        server.key_package("kp_charlie_1").unwrap().state,
        KeyPackageState::Available
    );
}

#[test]
fn multi_device_pending_invite_action_order_fuzz_keeps_server_roles_separate() {
    for seed in 1..=512 {
        run_multi_device_pending_invite_ordering(seed);
    }
}

fn run_multi_device_pending_invite_ordering(seed: u64) {
    const ROOM_ID: &str = "room_multi_device_fuzz";
    const GROUP_ID: &str = "mls_multi_device_fuzz";
    const STEPS: usize = 64;

    let mut server = DeliveryService::new();
    let bob_device = device("bob_npub", "bob_runtime");
    let alice_devices = [
        device("alice_npub", "alice_browser"),
        device("alice_npub", "alice_phone"),
        device("alice_npub", "alice_tablet"),
    ];
    server
        .create_or_get_direct_room(CreateDirectRoomRequest {
            room_id: ROOM_ID.to_string(),
            mls_group_id: GROUP_ID.to_string(),
            creator: bob_device.clone(),
            other_account_id: "alice_npub".to_string(),
        })
        .unwrap();
    for device in &alice_devices {
        upload_available_key_package(
            &mut server,
            device.clone(),
            &format!("kp_{}", device.device_id),
        );
    }
    let claimed_key_packages = server.claim_key_packages_for_account("alice_npub").unwrap();
    assert_eq!(claimed_key_packages.len(), alice_devices.len());

    let commit = envelope(
        ROOM_ID.to_string(),
        GROUP_ID.to_string(),
        bob_device.clone(),
        0,
        LogEntryKind::Commit,
        format!("multi_device_invite:{seed}").into_bytes(),
    );
    let commit_message_id = commit.message_id().unwrap();
    let accepted = server
        .submit_commit(SubmitCommitRequest {
            room_id: ROOM_ID.to_string(),
            sender: bob_device.clone(),
            expected_epoch: 0,
            envelope: commit,
            membership_delta: MembershipDeltaV1 {
                base_epoch: 0,
                post_commit_epoch: 1,
                commit_message_id,
                adds: claimed_key_packages
                    .iter()
                    .map(|claim| MembershipAddV1 {
                        device: claim.owner.clone(),
                        key_package_id: claim.key_package_id.clone(),
                        key_package_ref: claim.key_package_ref.clone(),
                        key_package_hash: claim.key_package_hash.clone(),
                        welcome_id: format!("welcome_{}", claim.owner.device_id),
                    })
                    .collect(),
                removes: vec![],
            },
            staged_welcomes: claimed_key_packages
                .iter()
                .map(|claim| staged_welcome(&format!("welcome_{}", claim.owner.device_id)))
                .collect(),
            idempotency_key: format!("invite_all_alice_devices_{seed}"),
        })
        .unwrap();
    assert_eq!(accepted.seq, 1);

    let mut rng = Lcg::new(seed);
    let mut acked = [false; 3];
    let mut cursors = [accepted.seq; 3];
    for step in 0..STEPS {
        let device_index = rng.next_usize(alice_devices.len());
        let device = alice_devices[device_index].clone();
        match rng.next_usize(4) {
            0 => {
                if acked[device_index] {
                    continue;
                }
                let welcome_id = format!("welcome_{}", device.device_id);
                let welcomes = server.claim_welcomes(&device);
                assert_eq!(welcomes.len(), 1);
                assert_eq!(welcomes[0].welcome_id, welcome_id);
                server.ack_welcome(&welcome_id, true).unwrap();
                acked[device_index] = true;
                assert!(server.room(ROOM_ID).unwrap().device_active_at_head(&device));
            }
            1 => {
                let request = sim_application_request(
                    ROOM_ID,
                    GROUP_ID,
                    bob_device.clone(),
                    format!("bob_msg_{seed}_{step}").as_bytes(),
                    &format!("bob_msg_{seed}_{step}"),
                );
                server.append_event(request).unwrap();
            }
            2 => {
                let before_seq = server.room(ROOM_ID).unwrap().last_seq;
                let request = sim_application_request(
                    ROOM_ID,
                    GROUP_ID,
                    device.clone(),
                    format!("alice_msg_{seed}_{step}_{device_index}").as_bytes(),
                    &format!("alice_msg_{seed}_{step}_{device_index}"),
                );
                let result = server.append_event(request);
                if acked[device_index] {
                    assert!(result.is_ok());
                } else {
                    assert_eq!(result.unwrap_err(), EngineError::SenderNotActive(device));
                    assert_eq!(server.room(ROOM_ID).unwrap().last_seq, before_seq);
                }
            }
            _ => {
                let page = server
                    .sync_events(ROOM_ID, &device, cursors[device_index])
                    .unwrap();
                assert!(page.entries.len() <= MAX_SYNC_PAGE_ENTRIES as usize);
                for entry in &page.entries {
                    assert!(entry.seq > cursors[device_index]);
                    assert_eq!(entry.kind, LogEntryKind::Application);
                }
                if let Some(last_entry) = page.entries.last() {
                    cursors[device_index] = last_entry.seq;
                }
            }
        }
    }

    let room = server.room(ROOM_ID).unwrap();
    let expected_visible_entries = (room.last_seq - accepted.seq) as usize;
    assert!(expected_visible_entries <= MAX_SYNC_PAGE_ENTRIES as usize);
    for (index, device) in alice_devices.iter().enumerate() {
        let page = server.sync_events(ROOM_ID, device, accepted.seq).unwrap();
        assert_eq!(page.entries.len(), expected_visible_entries);
        assert!(!page.has_more);
        if !acked[index] {
            let request = sim_application_request(
                ROOM_ID,
                GROUP_ID,
                device.clone(),
                b"still pending",
                &format!("pending_final_{seed}_{index}"),
            );
            assert_eq!(
                server.append_event(request).unwrap_err(),
                EngineError::SenderNotActive(device.clone())
            );
        }
    }
}

#[derive(Debug)]
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        assert!(seed > 0);
        Self {
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        assert!(bound > 0);
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state as usize) % bound
    }
}

fn sim_application_request(
    room_id: &str,
    group_id: &str,
    sender: DeviceRef,
    body: &[u8],
    idempotency_key: &str,
) -> AppendEventRequest {
    AppendEventRequest {
        room_id: room_id.to_string(),
        sender: sender.clone(),
        envelope: envelope(
            room_id.to_string(),
            group_id.to_string(),
            sender,
            1,
            LogEntryKind::Application,
            body.to_vec(),
        ),
        idempotency_key: idempotency_key.to_string(),
    }
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
    assert_eq!(welcomes[0].welcome_payload, b"welcome:welcome_bob_1");
    assert_eq!(welcomes[0].ratchet_tree_payload, b"tree:welcome_bob_1");
    world.server.ack_welcome("welcome_bob_1", true).unwrap();

    let room = world.server.room(&world.room_id).unwrap();
    assert!(room.device_active_at_head(&bob()));
}

#[test]
fn add_commit_requires_staged_welcome_bytes_before_mutation() {
    let mut world = SimWorld::direct_room().unwrap();
    world.upload_and_claim(bob(), "kp_bob_1").unwrap();
    let mut request = world
        .add_device_request(
            alice(),
            bob(),
            "kp_bob_1",
            "welcome_bob_1",
            0,
            "missing_welcome",
        )
        .unwrap();
    request.staged_welcomes.clear();

    let err = world.server.submit_commit(request).unwrap_err();

    assert_eq!(
        err,
        EngineError::MissingStagedWelcome("welcome_bob_1".to_string())
    );
    let room = world.server.room(&world.room_id).unwrap();
    assert_eq!(room.current_epoch, 0);
    assert_eq!(room.last_seq, 0);
    assert!(world.server.welcome("welcome_bob_1").is_none());
    assert_eq!(
        world.server.key_package("kp_bob_1").unwrap().state,
        KeyPackageState::Leased
    );
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
        .add_device_request(
            alice(),
            charlie(),
            "kp_bob_1",
            "welcome_charlie_reuse",
            1,
            "second",
        )
        .unwrap();
    let err = world.server.submit_commit(second).unwrap_err();

    assert_eq!(
        err,
        EngineError::KeyPackageUnavailable {
            key_package_id: "kp_bob_1".to_string(),
            state: KeyPackageState::Consumed
        }
    );
    assert!(world.server.welcome("welcome_charlie_reuse").is_none());
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

    let bob_page = world.server.sync_events(&world.room_id, &bob(), 1).unwrap();
    assert_eq!(bob_page.entries.len(), 1);
    assert_eq!(bob_page.entries[0].seq, accepted.seq);
    assert_eq!(bob_page.next_after_seq, accepted.seq);
    assert!(!bob_page.has_more);

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
    let err = world
        .server
        .append_event(world.app_message_request(bob(), 2, "stale send", "msg_stale_bob"))
        .unwrap_err();
    assert_eq!(err, EngineError::SenderNotActive(bob()));
    let stale_commit = world
        .remove_device_request(bob(), alice(), 2, "commit_stale_bob")
        .unwrap();
    let err = world.server.submit_commit(stale_commit).unwrap_err();
    assert_eq!(err, EngineError::SenderNotActive(bob()));

    let bob_page = world
        .server
        .sync_events(&world.room_id, &bob(), removal.seq)
        .unwrap();
    assert!(bob_page.entries.is_empty());
    assert_eq!(bob_page.next_after_seq, removal.seq + 1);
    assert!(!bob_page.has_more);
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
    let page = world.server.sync_events(&world.room_id, &bob(), 1).unwrap();

    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].seq, 2);
    assert_eq!(page.next_after_seq, 2);
    assert!(!page.has_more);
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
            key_package_payload: fake_key_package_payload("kp_charlie"),
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

#[test]
fn account_room_discovery_pages_current_devices_for_link_fanout() {
    let mut server = DeliveryService::new();
    server
        .create_room(CreateRoomRequest {
            room_id: "room_account_a".to_string(),
            mls_group_id: "mls_account_a".to_string(),
            creator: alice(),
        })
        .unwrap();
    server
        .create_room(CreateRoomRequest {
            room_id: "room_account_b".to_string(),
            mls_group_id: "mls_account_b".to_string(),
            creator: alice(),
        })
        .unwrap();
    server
        .create_room(CreateRoomRequest {
            room_id: "room_other_account".to_string(),
            mls_group_id: "mls_other_account".to_string(),
            creator: bob(),
        })
        .unwrap();

    let first_page = server
        .list_account_rooms(ListAccountRoomsRequest {
            account_id: alice().account_id,
            after_room_id: None,
            limit: 1,
        })
        .unwrap();
    assert_eq!(first_page.rooms.len(), 1);
    assert_eq!(first_page.rooms[0].room_id, "room_account_a");
    assert_eq!(first_page.rooms[0].devices.len(), 1);
    assert_eq!(first_page.rooms[0].devices[0].device, alice());
    assert!(first_page.rooms[0].devices[0].active);
    assert!(first_page.has_more);
    assert_eq!(
        first_page.next_after_room_id.as_deref(),
        Some("room_account_a")
    );

    let second_page = server
        .list_account_rooms(ListAccountRoomsRequest {
            account_id: alice().account_id,
            after_room_id: first_page.next_after_room_id,
            limit: 8,
        })
        .unwrap();
    assert_eq!(second_page.rooms.len(), 1);
    assert_eq!(second_page.rooms[0].room_id, "room_account_b");
    assert!(!second_page.has_more);
    assert_eq!(
        second_page.next_after_room_id.as_deref(),
        Some("room_account_b")
    );
}

#[test]
fn duplicate_current_or_pending_device_add_is_rejected_before_side_effects() {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .add_device_commit(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add_bob")
        .unwrap();
    world.upload_and_claim(bob(), "kp_bob_retry").unwrap();

    let duplicate = world
        .add_device_request(
            alice(),
            bob(),
            "kp_bob_retry",
            "welcome_bob_retry",
            1,
            "add_bob_retry",
        )
        .unwrap();
    assert!(matches!(
        world.server.submit_commit(duplicate).unwrap_err(),
        EngineError::DeviceAlreadyInRoom(device) if device == bob()
    ));
    assert_eq!(
        world.server.key_package("kp_bob_retry").unwrap().state,
        KeyPackageState::Leased
    );
    assert!(world.server.welcome("welcome_bob_retry").is_none());
}

#[test]
fn oversized_application_payload_is_rejected_without_log_entry() {
    let mut world = SimWorld::direct_room().unwrap();
    let request = AppendEventRequest {
        room_id: world.room_id.clone(),
        sender: alice(),
        envelope: envelope(
            world.room_id.clone(),
            world.group_id.clone(),
            alice(),
            0,
            LogEntryKind::Application,
            vec![0; MAX_ENVELOPE_PAYLOAD_BYTES as usize + 1],
        ),
        idempotency_key: "oversized_payload".to_string(),
    };

    let err = world.server.append_event(request).unwrap_err();

    assert!(matches!(
        err,
        EngineError::ProtocolLimit(ProtocolLimitError::BytesTooLong { field, .. })
            if field == "envelope.payload"
    ));
    assert_eq!(world.server.room(&world.room_id).unwrap().log.len(), 0);
}

#[test]
fn sync_events_returns_bounded_page() {
    let mut world = SimWorld::direct_room().unwrap();
    for index in 0..=MAX_SYNC_PAGE_ENTRIES {
        world
            .server
            .append_event(world.app_message_request(
                alice(),
                0,
                &format!("small_{index}"),
                &format!("msg_{index}"),
            ))
            .unwrap();
    }

    let page = world
        .server
        .sync_events(&world.room_id, &alice(), 0)
        .unwrap();

    assert_eq!(page.entries.len(), MAX_SYNC_PAGE_ENTRIES as usize);
    assert_eq!(page.entries.first().unwrap().seq, 1);
    assert_eq!(
        page.entries.last().unwrap().seq,
        u64::from(MAX_SYNC_PAGE_ENTRIES)
    );
    assert_eq!(page.next_after_seq, u64::from(MAX_SYNC_PAGE_ENTRIES));
    assert!(page.has_more);
}

#[test]
fn duplicate_message_id_with_new_idempotency_key_is_rejected() {
    let mut world = SimWorld::direct_room().unwrap();
    let first = world.app_message_request(alice(), 0, "same ciphertext", "msg_first");
    let duplicate = world.app_message_request(alice(), 0, "same ciphertext", "msg_second");
    let message_id = first.envelope.message_id().unwrap();

    world.server.append_event(first).unwrap();
    let err = world.server.append_event(duplicate).unwrap_err();

    assert_eq!(err, EngineError::DuplicateMessageId(message_id));
    assert_eq!(world.server.room(&world.room_id).unwrap().log.len(), 1);
}

#[test]
fn idempotency_capacity_rejects_new_mutations_but_allows_replay() {
    let mut world = SimWorld::direct_room().unwrap();
    let first = world.app_message_request(alice(), 0, "body_0", "msg_0");
    let first_result = world.server.append_event(first.clone()).unwrap();

    for index in 1..MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE {
        world
            .server
            .append_event(world.app_message_request(
                alice(),
                0,
                &format!("body_{index}"),
                &format!("msg_{index}"),
            ))
            .unwrap();
    }

    let replayed = world.server.append_event(first).unwrap();
    let overflow = world.app_message_request(
        alice(),
        0,
        "body_overflow",
        &format!("msg_{MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE}"),
    );
    let err = world.server.append_event(overflow).unwrap_err();

    assert_eq!(replayed, first_result);
    assert!(matches!(
        err,
        EngineError::IdempotencyCapacityExceeded { room_id, sender, max_records }
            if room_id == world.room_id
                && sender == alice()
                && max_records == MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE
    ));
    assert_eq!(
        world.server.room(&world.room_id).unwrap().log.len(),
        MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE as usize
    );
}

#[test]
fn direct_room_rejects_too_many_devices_for_one_account() {
    let mut server = DeliveryService::new();
    let room_id = server
        .create_or_get_direct_room(CreateDirectRoomRequest {
            room_id: "direct_ab".to_string(),
            mls_group_id: "mls_ab".to_string(),
            creator: alice(),
            other_account_id: bob().account_id,
        })
        .unwrap();

    for index in 0..MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT {
        let target = device("bob_npub", format!("bob_device_{index}"));
        let key_package_id = format!("kp_bob_{index}");
        server
            .upload_key_package(UploadKeyPackageRequest {
                key_package_id: key_package_id.clone(),
                owner: target.clone(),
                key_package_ref: format!("ref_{key_package_id}"),
                key_package_hash: format!("hash_{key_package_id}"),
                key_package_payload: fake_key_package_payload(&key_package_id),
            })
            .unwrap();
        server.claim_key_package(&key_package_id).unwrap();
        let commit = envelope(
            room_id.clone(),
            "mls_ab",
            alice(),
            u64::from(index),
            LogEntryKind::Commit,
            format!("add_bob_{index}").into_bytes(),
        );
        let commit_message_id = commit.message_id().unwrap();
        server
            .submit_commit(SubmitCommitRequest {
                room_id: room_id.clone(),
                sender: alice(),
                expected_epoch: u64::from(index),
                envelope: commit,
                membership_delta: MembershipDeltaV1 {
                    base_epoch: u64::from(index),
                    post_commit_epoch: u64::from(index) + 1,
                    commit_message_id,
                    adds: vec![MembershipAddV1 {
                        device: target,
                        key_package_id: key_package_id.clone(),
                        key_package_ref: format!("ref_{key_package_id}"),
                        key_package_hash: format!("hash_{key_package_id}"),
                        welcome_id: format!("welcome_bob_{index}"),
                    }],
                    removes: vec![],
                },
                idempotency_key: format!("add_bob_{index}"),
                staged_welcomes: vec![staged_welcome(&format!("welcome_bob_{index}"))],
            })
            .unwrap();
    }

    let overflow = device("bob_npub", "bob_device_overflow");
    server
        .upload_key_package(UploadKeyPackageRequest {
            key_package_id: "kp_bob_overflow".to_string(),
            owner: overflow.clone(),
            key_package_ref: "ref_kp_bob_overflow".to_string(),
            key_package_hash: "hash_kp_bob_overflow".to_string(),
            key_package_payload: fake_key_package_payload("kp_bob_overflow"),
        })
        .unwrap();
    server.claim_key_package("kp_bob_overflow").unwrap();
    let commit = envelope(
        room_id.clone(),
        "mls_ab",
        alice(),
        u64::from(MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT),
        LogEntryKind::Commit,
        b"add_bob_overflow".to_vec(),
    );
    let commit_message_id = commit.message_id().unwrap();
    let err = server
        .submit_commit(SubmitCommitRequest {
            room_id,
            sender: alice(),
            expected_epoch: u64::from(MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT),
            envelope: commit,
            membership_delta: MembershipDeltaV1 {
                base_epoch: u64::from(MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT),
                post_commit_epoch: u64::from(MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT) + 1,
                commit_message_id,
                adds: vec![MembershipAddV1 {
                    device: overflow,
                    key_package_id: "kp_bob_overflow".to_string(),
                    key_package_ref: "ref_kp_bob_overflow".to_string(),
                    key_package_hash: "hash_kp_bob_overflow".to_string(),
                    welcome_id: "welcome_bob_overflow".to_string(),
                }],
                removes: vec![],
            },
            idempotency_key: "add_bob_overflow".to_string(),
            staged_welcomes: vec![staged_welcome("welcome_bob_overflow")],
        })
        .unwrap_err();

    assert!(matches!(
        err,
        EngineError::ProtocolLimit(ProtocolLimitError::TooManyItems { field, .. })
            if field == "direct_room.devices_per_account"
    ));
}

#[test]
fn group_room_rejects_too_many_devices_for_one_account() {
    let mut world = SimWorld::direct_room().unwrap();

    for index in 0..(MAX_ACCOUNT_DEVICES_PER_ROOM - 1) {
        let target = device("alice_npub", format!("alice_extra_{index}"));
        let key_package_id = format!("kp_alice_extra_{index}");
        world
            .upload_and_claim(target.clone(), &key_package_id)
            .unwrap();
        let request = world
            .add_device_request(
                alice(),
                target,
                &key_package_id,
                &format!("welcome_alice_extra_{index}"),
                u64::from(index),
                &format!("add_alice_extra_{index}"),
            )
            .unwrap();
        world.server.submit_commit(request).unwrap();
    }

    let overflow = device("alice_npub", "alice_extra_overflow");
    world
        .upload_and_claim(overflow.clone(), "kp_alice_extra_overflow")
        .unwrap();
    let request = world
        .add_device_request(
            alice(),
            overflow,
            "kp_alice_extra_overflow",
            "welcome_alice_extra_overflow",
            u64::from(MAX_ACCOUNT_DEVICES_PER_ROOM - 1),
            "add_alice_extra_overflow",
        )
        .unwrap();
    let err = world.server.submit_commit(request).unwrap_err();

    assert!(matches!(
        err,
        EngineError::ProtocolLimit(ProtocolLimitError::TooManyItems { field, .. })
            if field == "room.devices_per_account"
    ));
    assert!(
        world
            .server
            .welcome("welcome_alice_extra_overflow")
            .is_none()
    );
}
