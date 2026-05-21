use finitechat_client::{ClientError, FiniteChatDevice, FiniteChatDeviceConfig};
use finitechat_engine::DeliveryService;
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
