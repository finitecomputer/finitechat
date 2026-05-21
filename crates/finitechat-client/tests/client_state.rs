use finitechat_client::{ClientError, FiniteChatDevice, FiniteChatDeviceConfig};
use finitechat_engine::{AppendEventRequest, DeliveryService, EngineError, envelope};
use finitechat_mls::{NOSTR_SECRET_KEY_BYTES, NostrSecretKey};
use finitechat_proto::LogEntryKind;

const ALICE_ACCOUNT_SECRET_BYTES: [u8; NOSTR_SECRET_KEY_BYTES] = [17; NOSTR_SECRET_KEY_BYTES];
const BOB_ACCOUNT_SECRET_BYTES: [u8; NOSTR_SECRET_KEY_BYTES] = [19; NOSTR_SECRET_KEY_BYTES];
const ROOM_ID: &str = "room_client_direct";
const MLS_GROUP_ID: &str = "mls_client_direct";
const BOB_KEY_PACKAGE_ID: &str = "kp_bob_client_1";
const BOB_WELCOME_ID: &str = "welcome_bob_client_1";
const NOW: u64 = 1_800_000_000;

#[test]
fn client_state_machine_adds_device_and_decrypts_application_message() {
    let mut alice = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser");
    let mut bob = test_device(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let mut server = DeliveryService::new();

    server
        .create_or_get_direct_room(alice.create_direct_room_request(
            ROOM_ID,
            MLS_GROUP_ID,
            bob.device_ref().account_id.clone(),
        ))
        .unwrap();
    alice.create_group_state(ROOM_ID, MLS_GROUP_ID).unwrap();
    server
        .upload_key_package(bob.upload_key_package_request(BOB_KEY_PACKAGE_ID).unwrap())
        .unwrap();
    let claimed_key_package = server.claim_key_package(BOB_KEY_PACKAGE_ID).unwrap();

    let prepared = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_key_package,
            BOB_WELCOME_ID,
            "client_add_bob",
        )
        .unwrap();
    assert_eq!(alice.group_epoch(ROOM_ID).unwrap(), 0);
    assert!(alice.has_pending_commit(ROOM_ID).unwrap());
    assert!(matches!(
        alice.create_application_request(ROOM_ID, b"too early", "client_too_early"),
        Err(ClientError::PendingCommitMustBeMerged(room_id)) if room_id == ROOM_ID
    ));

    let accepted = server.submit_commit(prepared.request).unwrap();
    let alice_page = server.sync_events(ROOM_ID, alice.device_ref(), 0).unwrap();
    assert_eq!(alice_page.entries.len(), 1);
    assert_eq!(alice_page.entries[0].kind, LogEntryKind::Commit);
    alice
        .merge_pending_commit_from_log(ROOM_ID, &alice_page.entries, &prepared.message_id)
        .unwrap();
    assert_eq!(alice.group_epoch(ROOM_ID).unwrap(), 1);
    assert!(!alice.has_pending_commit(ROOM_ID).unwrap());

    let claimed_welcomes = server.claim_welcomes(bob.device_ref());
    assert_eq!(claimed_welcomes.len(), 1);
    bob.activate_welcome(
        ROOM_ID,
        &claimed_welcomes[0].welcome_payload,
        &claimed_welcomes[0].ratchet_tree_payload,
    )
    .unwrap();
    server.ack_welcome(BOB_WELCOME_ID, true).unwrap();
    assert_eq!(bob.group_epoch(ROOM_ID).unwrap(), 1);
    assert_eq!(
        alice
            .verified_member_count(ROOM_ID, bob.device_ref())
            .unwrap(),
        1
    );
    assert_eq!(
        bob.verified_member_count(ROOM_ID, alice.device_ref())
            .unwrap(),
        1
    );

    let plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"run tests"}}"#;
    let app_request = alice
        .create_application_request(ROOM_ID, plaintext, "client_app_message")
        .unwrap();
    let appended = server.append_event(app_request).unwrap();
    assert_eq!(appended.seq, accepted.seq + 1);

    let bob_page = server
        .sync_events(ROOM_ID, bob.device_ref(), accepted.seq)
        .unwrap();
    assert_eq!(bob_page.entries.len(), 1);
    let decrypted = bob
        .decrypt_application_entry(ROOM_ID, &bob_page.entries[0])
        .unwrap();
    assert_eq!(decrypted, plaintext);
}

#[test]
fn multi_device_invite_late_joiner_catches_up_to_new_messages() {
    let mut bob = test_device(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let mut alice_browser = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser");
    let mut alice_phone = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_phone");
    let mut server = DeliveryService::new();

    server
        .create_or_get_direct_room(bob.create_direct_room_request(
            ROOM_ID,
            MLS_GROUP_ID,
            alice_browser.device_ref().account_id.clone(),
        ))
        .unwrap();
    bob.create_group_state(ROOM_ID, MLS_GROUP_ID).unwrap();
    server
        .upload_key_package(
            alice_browser
                .upload_key_package_request("kp_alice_browser_1")
                .unwrap(),
        )
        .unwrap();
    server
        .upload_key_package(
            alice_phone
                .upload_key_package_request("kp_alice_phone_1")
                .unwrap(),
        )
        .unwrap();

    let claimed_key_packages = server
        .claim_key_packages_for_account(&alice_browser.device_ref().account_id)
        .unwrap();
    assert_eq!(claimed_key_packages.len(), 2);
    let welcome_ids = claimed_key_packages
        .iter()
        .map(|claim| format!("welcome_{}", claim.owner.device_id))
        .collect::<Vec<_>>();
    let browser_welcome_id = welcome_ids
        .iter()
        .zip(&claimed_key_packages)
        .find(|(_, claim)| claim.owner.device_id == "alice_browser")
        .map(|(welcome_id, _)| welcome_id.clone())
        .unwrap();
    let phone_welcome_id = welcome_ids
        .iter()
        .zip(&claimed_key_packages)
        .find(|(_, claim)| claim.owner.device_id == "alice_phone")
        .map(|(welcome_id, _)| welcome_id.clone())
        .unwrap();

    let prepared = bob
        .prepare_add_members_commit(
            ROOM_ID,
            &claimed_key_packages,
            &welcome_ids,
            "invite_all_alice_devices",
        )
        .unwrap();
    assert_eq!(prepared.request.membership_delta.adds.len(), 2);
    assert_eq!(prepared.request.staged_welcomes.len(), 2);
    assert_eq!(
        prepared.request.staged_welcomes[0].welcome_payload,
        prepared.request.staged_welcomes[1].welcome_payload
    );
    let accepted = server.submit_commit(prepared.request).unwrap();
    let bob_page = server.sync_events(ROOM_ID, bob.device_ref(), 0).unwrap();
    bob.merge_pending_commit_from_log(ROOM_ID, &bob_page.entries, &prepared.message_id)
        .unwrap();
    assert_eq!(bob.group_epoch(ROOM_ID).unwrap(), 1);

    let browser_join_seq = claim_and_activate(&mut server, &mut alice_browser, &browser_welcome_id);
    assert_eq!(browser_join_seq, accepted.seq);

    let first_plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"first"}}"#;
    let first = bob
        .create_application_request(ROOM_ID, first_plaintext, "bob_first")
        .unwrap();
    let first_accepted = server.append_event(first).unwrap();
    assert_eq!(first_accepted.seq, accepted.seq + 1);

    let browser_page = server
        .sync_events(ROOM_ID, alice_browser.device_ref(), accepted.seq)
        .unwrap();
    assert_eq!(browser_page.entries.len(), 1);
    assert_eq!(
        alice_browser
            .decrypt_application_entry(ROOM_ID, &browser_page.entries[0])
            .unwrap(),
        first_plaintext
    );

    let second_plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"second"}}"#;
    let second = bob
        .create_application_request(ROOM_ID, second_plaintext, "bob_second")
        .unwrap();
    let second_accepted = server.append_event(second).unwrap();

    assert!(matches!(
        alice_phone.create_application_request(ROOM_ID, b"pending", "phone_pending"),
        Err(ClientError::GroupNotFound(room_id)) if room_id == ROOM_ID
    ));
    let pending_phone_page = server
        .sync_events(ROOM_ID, alice_phone.device_ref(), accepted.seq)
        .unwrap();
    assert_eq!(pending_phone_page.entries.len(), 2);
    assert_eq!(
        server
            .append_event(fake_application_request(
                alice_phone.device_ref().clone(),
                1,
                "phone_before_ack"
            ))
            .unwrap_err(),
        EngineError::SenderNotActive(alice_phone.device_ref().clone())
    );

    let phone_join_seq = claim_and_activate(&mut server, &mut alice_phone, &phone_welcome_id);
    assert_eq!(phone_join_seq, accepted.seq);
    assert_eq!(
        alice_phone
            .decrypt_application_entry(ROOM_ID, &pending_phone_page.entries[0])
            .unwrap(),
        first_plaintext
    );
    assert_eq!(
        alice_phone
            .decrypt_application_entry(ROOM_ID, &pending_phone_page.entries[1])
            .unwrap(),
        second_plaintext
    );

    let third_plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"third"}}"#;
    let third = bob
        .create_application_request(ROOM_ID, third_plaintext, "bob_third")
        .unwrap();
    server.append_event(third).unwrap();
    let browser_page = server
        .sync_events(ROOM_ID, alice_browser.device_ref(), second_accepted.seq)
        .unwrap();
    let phone_page = server
        .sync_events(ROOM_ID, alice_phone.device_ref(), second_accepted.seq)
        .unwrap();
    assert_eq!(
        alice_browser
            .decrypt_application_entry(ROOM_ID, &browser_page.entries[0])
            .unwrap(),
        third_plaintext
    );
    assert_eq!(
        alice_phone
            .decrypt_application_entry(ROOM_ID, &phone_page.entries[0])
            .unwrap(),
        third_plaintext
    );
}

#[test]
fn client_refuses_to_merge_pending_commit_before_server_observation() {
    let mut alice = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser");
    let bob = test_device(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let mut server = DeliveryService::new();

    server
        .create_or_get_direct_room(alice.create_direct_room_request(
            ROOM_ID,
            MLS_GROUP_ID,
            bob.device_ref().account_id.clone(),
        ))
        .unwrap();
    alice.create_group_state(ROOM_ID, MLS_GROUP_ID).unwrap();
    server
        .upload_key_package(bob.upload_key_package_request(BOB_KEY_PACKAGE_ID).unwrap())
        .unwrap();
    let claimed_key_package = server.claim_key_package(BOB_KEY_PACKAGE_ID).unwrap();
    let prepared = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_key_package,
            BOB_WELCOME_ID,
            "client_add_bob",
        )
        .unwrap();

    let err = alice
        .merge_pending_commit_from_log(ROOM_ID, &[], &prepared.message_id)
        .unwrap_err();

    assert!(matches!(err, ClientError::PendingCommitNotObserved(_)));
    assert_eq!(alice.group_epoch(ROOM_ID).unwrap(), 0);
    assert!(alice.has_pending_commit(ROOM_ID).unwrap());
}

#[test]
fn client_rejects_invalid_invite_request_before_local_pending_commit() {
    let mut alice = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser");
    let bob = test_device(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let mut server = DeliveryService::new();

    server
        .create_or_get_direct_room(alice.create_direct_room_request(
            ROOM_ID,
            MLS_GROUP_ID,
            bob.device_ref().account_id.clone(),
        ))
        .unwrap();
    alice.create_group_state(ROOM_ID, MLS_GROUP_ID).unwrap();
    server
        .upload_key_package(bob.upload_key_package_request(BOB_KEY_PACKAGE_ID).unwrap())
        .unwrap();
    let claimed_key_package = server.claim_key_package(BOB_KEY_PACKAGE_ID).unwrap();

    let err = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_key_package,
            BOB_WELCOME_ID,
            "x".repeat(129),
        )
        .unwrap_err();

    assert!(matches!(err, ClientError::ProtocolLimit(_)));
    assert_eq!(alice.group_epoch(ROOM_ID).unwrap(), 0);
    assert!(!alice.has_pending_commit(ROOM_ID).unwrap());
}

#[test]
fn client_rejects_tampered_ratchet_tree_before_ack() {
    let mut alice = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser");
    let mut bob = test_device(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let mut server = DeliveryService::new();

    server
        .create_or_get_direct_room(alice.create_direct_room_request(
            ROOM_ID,
            MLS_GROUP_ID,
            bob.device_ref().account_id.clone(),
        ))
        .unwrap();
    alice.create_group_state(ROOM_ID, MLS_GROUP_ID).unwrap();
    server
        .upload_key_package(bob.upload_key_package_request(BOB_KEY_PACKAGE_ID).unwrap())
        .unwrap();
    let claimed_key_package = server.claim_key_package(BOB_KEY_PACKAGE_ID).unwrap();
    let prepared = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_key_package,
            BOB_WELCOME_ID,
            "client_add_bob",
        )
        .unwrap();
    server.submit_commit(prepared.request).unwrap();

    let claimed_welcomes = server.claim_welcomes(bob.device_ref());
    let mut tampered_tree = claimed_welcomes[0].ratchet_tree_payload.clone();
    let last = tampered_tree.len() - 1;
    tampered_tree[last] ^= 0x01;

    let err = bob
        .activate_welcome(
            ROOM_ID,
            &claimed_welcomes[0].welcome_payload,
            &tampered_tree,
        )
        .unwrap_err();

    assert!(matches!(
        err,
        ClientError::ParseRatchetTree | ClientError::StageWelcome | ClientError::ActivateWelcome
    ));
}

fn test_device(
    account_secret_bytes: [u8; NOSTR_SECRET_KEY_BYTES],
    device_id: &str,
) -> FiniteChatDevice {
    FiniteChatDevice::new(FiniteChatDeviceConfig {
        account_secret_key: NostrSecretKey::from_bytes(account_secret_bytes).unwrap(),
        device_id: device_id.to_string(),
        now_unix_seconds: NOW,
        credential_not_before_unix_seconds: NOW - 60,
        credential_not_after_unix_seconds: NOW + 60,
    })
    .unwrap()
}

fn claim_and_activate(
    server: &mut DeliveryService,
    device: &mut FiniteChatDevice,
    welcome_id: &str,
) -> u64 {
    let claimed_welcomes = server.claim_welcomes(device.device_ref());
    let welcome = claimed_welcomes
        .into_iter()
        .find(|welcome| welcome.welcome_id == welcome_id)
        .unwrap();
    device
        .activate_welcome(
            ROOM_ID,
            &welcome.welcome_payload,
            &welcome.ratchet_tree_payload,
        )
        .unwrap();
    server.ack_welcome(welcome_id, true).unwrap();
    welcome.commit_seq
}

fn fake_application_request(
    sender: finitechat_proto::DeviceRef,
    epoch: u64,
    idempotency_key: &str,
) -> AppendEventRequest {
    AppendEventRequest {
        room_id: ROOM_ID.to_string(),
        sender: sender.clone(),
        envelope: envelope(
            ROOM_ID.to_string(),
            MLS_GROUP_ID.to_string(),
            sender,
            epoch,
            LogEntryKind::Application,
            b"not an MLS ciphertext".to_vec(),
        ),
        idempotency_key: idempotency_key.to_string(),
    }
}
