use finitechat_client::{
    AppliedLogEntry, ClientError, ClientStoreError, FiniteChatDevice, FiniteChatDeviceConfig,
    SqliteClientStore, SqliteClientStoreOptions,
};
use finitechat_engine::{
    AppendEventRequest, CreateRoomRequest, DeliveryService, EngineError, EventAccepted, envelope,
};
use finitechat_mls::{NOSTR_SECRET_KEY_BYTES, NostrSecretKey};
use finitechat_proto::{KeyPackageState, LogEntryKind};
use rusqlite::{Connection, params};

const ALICE_ACCOUNT_SECRET_BYTES: [u8; NOSTR_SECRET_KEY_BYTES] = [17; NOSTR_SECRET_KEY_BYTES];
const BOB_ACCOUNT_SECRET_BYTES: [u8; NOSTR_SECRET_KEY_BYTES] = [19; NOSTR_SECRET_KEY_BYTES];
const CHARLIE_ACCOUNT_SECRET_BYTES: [u8; NOSTR_SECRET_KEY_BYTES] = [23; NOSTR_SECRET_KEY_BYTES];
const DANA_ACCOUNT_SECRET_BYTES: [u8; NOSTR_SECRET_KEY_BYTES] = [29; NOSTR_SECRET_KEY_BYTES];
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
fn sqlite_client_state_survives_restart_for_late_multi_device_catch_up() {
    let dir = tempfile::tempdir().unwrap();
    let bob_config = test_config(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let browser_config = test_config(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser");
    let phone_config = test_config(ALICE_ACCOUNT_SECRET_BYTES, "alice_phone");
    let mut bob_store = sqlite_client_store(dir.path().join("bob.sqlite3"), &bob_config);
    let mut browser_store =
        sqlite_client_store(dir.path().join("alice_browser.sqlite3"), &browser_config);
    let mut phone_store =
        sqlite_client_store(dir.path().join("alice_phone.sqlite3"), &phone_config);
    let mut bob = FiniteChatDevice::new(bob_config.clone()).unwrap();
    let mut alice_browser = FiniteChatDevice::new(browser_config.clone()).unwrap();
    let mut alice_phone = FiniteChatDevice::new(phone_config.clone()).unwrap();
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
                .upload_key_package_request("kp_restart_browser_1")
                .unwrap(),
        )
        .unwrap();
    server
        .upload_key_package(
            alice_phone
                .upload_key_package_request("kp_restart_phone_1")
                .unwrap(),
        )
        .unwrap();

    let claimed_key_packages = server
        .claim_key_packages_for_account(&alice_browser.device_ref().account_id)
        .unwrap();
    assert_eq!(claimed_key_packages.len(), 2);
    let welcome_ids = claimed_key_packages
        .iter()
        .map(|claim| format!("welcome_restart_{}", claim.owner.device_id))
        .collect::<Vec<_>>();
    let browser_welcome_id = welcome_id_for(&claimed_key_packages, &welcome_ids, "alice_browser");
    let phone_welcome_id = welcome_id_for(&claimed_key_packages, &welcome_ids, "alice_phone");
    let prepared = bob
        .prepare_add_members_commit(
            ROOM_ID,
            &claimed_key_packages,
            &welcome_ids,
            "restart_invite_all_alice_devices",
        )
        .unwrap();
    let accepted = server.submit_commit(prepared.request).unwrap();
    let bob_page = server.sync_events(ROOM_ID, bob.device_ref(), 0).unwrap();
    bob.merge_pending_commit_from_log(ROOM_ID, &bob_page.entries, &prepared.message_id)
        .unwrap();
    bob_store.save_device_state(&bob).unwrap();
    let mut bob = bob_store.load_device(bob_config.clone()).unwrap();
    assert_eq!(bob.group_epoch(ROOM_ID).unwrap(), 1);

    claim_and_activate(&mut server, &mut alice_browser, &browser_welcome_id);
    browser_store.save_device_state(&alice_browser).unwrap();
    let mut alice_browser = browser_store.load_device(browser_config).unwrap();

    let first_plaintext =
        br#"{"type":"finitecomputer.command.v1","body":{"text":"after bob restart"}}"#;
    let first = bob
        .create_application_request(ROOM_ID, first_plaintext, "restart_bob_first")
        .unwrap();
    let first_accepted = server.append_event(first).unwrap();
    let browser_page = server
        .sync_events(ROOM_ID, alice_browser.device_ref(), accepted.seq)
        .unwrap();
    assert_eq!(
        alice_browser
            .decrypt_application_entry(ROOM_ID, &browser_page.entries[0])
            .unwrap(),
        first_plaintext
    );

    let pending_phone_page = server
        .sync_events(ROOM_ID, alice_phone.device_ref(), accepted.seq)
        .unwrap();
    assert_eq!(pending_phone_page.entries.len(), 1);
    assert_eq!(
        server
            .append_event(fake_application_request(
                alice_phone.device_ref().clone(),
                1,
                "restart_phone_before_ack",
            ))
            .unwrap_err(),
        EngineError::SenderNotActive(alice_phone.device_ref().clone())
    );

    claim_and_activate(&mut server, &mut alice_phone, &phone_welcome_id);
    phone_store.save_device_state(&alice_phone).unwrap();
    let mut alice_phone = phone_store.load_device(phone_config).unwrap();
    assert_eq!(
        alice_phone
            .decrypt_application_entry(ROOM_ID, &pending_phone_page.entries[0])
            .unwrap(),
        first_plaintext
    );

    let second_plaintext =
        br#"{"type":"finitecomputer.command.v1","body":{"text":"after phone restart"}}"#;
    let second = bob
        .create_application_request(ROOM_ID, second_plaintext, "restart_bob_second")
        .unwrap();
    server.append_event(second).unwrap();
    bob_store.save_device_state(&bob).unwrap();
    let mut bob = bob_store.load_device(bob_config).unwrap();
    let phone_page = server
        .sync_events(ROOM_ID, alice_phone.device_ref(), first_accepted.seq)
        .unwrap();
    assert_eq!(
        alice_phone
            .decrypt_application_entry(ROOM_ID, &phone_page.entries[0])
            .unwrap(),
        second_plaintext
    );
    let bob_third = bob
        .create_application_request(
            ROOM_ID,
            br#"{"type":"finitecomputer.command.v1","body":{"text":"after second bob restart"}}"#,
            "restart_bob_third",
        )
        .unwrap();
    server.append_event(bob_third).unwrap();
}

#[test]
fn sqlite_client_store_encrypts_state_and_rejects_wrong_or_tampered_key_material() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("encrypted.sqlite3");
    let config = test_config(BOB_ACCOUNT_SECRET_BYTES, "bob_secure_store");
    let mut device = FiniteChatDevice::new(config.clone()).unwrap();
    device
        .create_group_state("room_secure_store", "mls_secure_store")
        .unwrap();
    let exported_state = device.export_state().unwrap();
    let mut store = sqlite_client_store(&path, &config);

    store.save_device_state(&device).unwrap();
    let conn = Connection::open(&path).unwrap();
    let legacy_tables: u64 = conn
        .query_row(
            r#"
            SELECT COUNT(*)
            FROM sqlite_master
            WHERE type = 'table'
              AND name IN ('client_profiles', 'client_rooms', 'client_openmls_storage')
            "#,
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_tables, 0);

    let (nonce, ciphertext): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            r#"
            SELECT nonce, ciphertext
            FROM client_device_states
            WHERE account_id = ?1 AND device_id = ?2
            "#,
            params![
                hex_lower(config.account_secret_key.public_key().as_bytes()),
                &config.device_id,
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(nonce.len(), 12);
    assert!(
        !contains_subsequence(&ciphertext, &exported_state.credential_identity),
        "credential identity should only appear inside encrypted state"
    );
    let storage_value = exported_state
        .openmls_storage_records
        .iter()
        .find(|record| record.value.len() >= 16)
        .expect("OpenMLS should persist at least one non-trivial secret row");
    assert!(
        !contains_subsequence(&ciphertext, &storage_value.value),
        "OpenMLS storage values should only appear inside encrypted state"
    );
    drop(conn);

    let wrong_config = test_config(ALICE_ACCOUNT_SECRET_BYTES, "bob_secure_store");
    let wrong_store = SqliteClientStore::open(
        &path,
        SqliteClientStoreOptions::from_nostr_secret(
            &wrong_config.account_secret_key,
            &config.device_id,
        )
        .unwrap(),
    )
    .unwrap();
    let wrong_key_error = match wrong_store.load_device(config.clone()) {
        Ok(_) => panic!("wrong local store key should not decrypt client state"),
        Err(error) => error,
    };
    assert!(matches!(wrong_key_error, ClientStoreError::DecryptState));

    let mut tampered = ciphertext;
    tampered[0] ^= 0x01;
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        r#"
        UPDATE client_device_states
        SET ciphertext = ?1
        WHERE account_id = ?2 AND device_id = ?3
        "#,
        params![
            tampered,
            hex_lower(config.account_secret_key.public_key().as_bytes()),
            &config.device_id,
        ],
    )
    .unwrap();
    drop(conn);
    let tamper_error = match store.load_device(config) {
        Ok(_) => panic!("tampered local store ciphertext should not decrypt"),
        Err(error) => error,
    };
    assert!(matches!(tamper_error, ClientStoreError::DecryptState));
}

#[test]
fn sqlite_client_welcome_activation_is_durable_before_server_ack() {
    let dir = tempfile::tempdir().unwrap();
    let bob_config = test_config(BOB_ACCOUNT_SECRET_BYTES, "bob_welcome_resume");
    let mut bob_store = sqlite_client_store(dir.path().join("bob.sqlite3"), &bob_config);
    let mut alice = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_welcome_resume");
    let mut bob = FiniteChatDevice::new(bob_config.clone()).unwrap();
    let mut server = DeliveryService::new();

    server
        .create_room(CreateRoomRequest {
            room_id: ROOM_ID.to_string(),
            mls_group_id: MLS_GROUP_ID.to_string(),
            creator: alice.device_ref().clone(),
        })
        .unwrap();
    alice.create_group_state(ROOM_ID, MLS_GROUP_ID).unwrap();
    server
        .upload_key_package(
            bob.upload_key_package_request("kp_welcome_resume_bob")
                .unwrap(),
        )
        .unwrap();
    let claimed_key_package = server.claim_key_package("kp_welcome_resume_bob").unwrap();
    let prepared = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_key_package,
            "welcome_resume_bob",
            "add_resume_bob",
        )
        .unwrap();
    let accepted = server.submit_commit(prepared.request).unwrap();
    let alice_page = server.sync_events(ROOM_ID, alice.device_ref(), 0).unwrap();
    alice
        .merge_pending_commit_from_log(ROOM_ID, &alice_page.entries, &prepared.message_id)
        .unwrap();

    let claimed_welcomes = server.claim_welcomes(bob.device_ref());
    let welcome = claimed_welcomes
        .iter()
        .find(|welcome| welcome.welcome_id == "welcome_resume_bob")
        .unwrap();
    bob_store
        .activate_welcome_and_save(
            &mut bob,
            ROOM_ID,
            &welcome.welcome_payload,
            &welcome.ratchet_tree_payload,
            welcome.commit_seq,
        )
        .unwrap();
    assert_eq!(welcome.commit_seq, accepted.seq);

    let mut bob = bob_store.load_device(bob_config.clone()).unwrap();
    assert_eq!(bob.group_epoch(ROOM_ID).unwrap(), 1);
    assert_eq!(bob.last_applied_seq(ROOM_ID).unwrap(), accepted.seq);
    server.ack_welcome("welcome_resume_bob", true).unwrap();

    let plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"after ack resume"}}"#;
    let request = alice
        .create_application_request(ROOM_ID, plaintext, "resume_after_ack")
        .unwrap();
    let accepted_message = server.append_event(request).unwrap();
    let page = server
        .sync_events(
            ROOM_ID,
            bob.device_ref(),
            bob.last_applied_seq(ROOM_ID).unwrap(),
        )
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(
        bob_store
            .apply_log_entry_and_save(&mut bob, ROOM_ID, &page.entries[0])
            .unwrap(),
        Some(AppliedLogEntry::Application(plaintext.to_vec()))
    );

    let bob = bob_store.load_device(bob_config).unwrap();
    assert_eq!(bob.last_applied_seq(ROOM_ID).unwrap(), accepted_message.seq);
}

#[test]
fn sqlite_client_apply_log_entry_persists_cursor_and_skips_replay_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let bob_config = test_config(BOB_ACCOUNT_SECRET_BYTES, "bob_sync_resume");
    let mut bob_store = sqlite_client_store(dir.path().join("bob.sqlite3"), &bob_config);
    let mut alice = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_sync_resume");
    let mut bob = FiniteChatDevice::new(bob_config.clone()).unwrap();
    let charlie = test_device(CHARLIE_ACCOUNT_SECRET_BYTES, "charlie_sync_resume");
    let mut server = DeliveryService::new();

    server
        .create_room(CreateRoomRequest {
            room_id: ROOM_ID.to_string(),
            mls_group_id: MLS_GROUP_ID.to_string(),
            creator: alice.device_ref().clone(),
        })
        .unwrap();
    alice.create_group_state(ROOM_ID, MLS_GROUP_ID).unwrap();
    server
        .upload_key_package(bob.upload_key_package_request("kp_sync_bob").unwrap())
        .unwrap();
    let claimed_bob = server.claim_key_package("kp_sync_bob").unwrap();
    let add_bob = alice
        .prepare_add_member_commit(ROOM_ID, &claimed_bob, "welcome_sync_bob", "add_sync_bob")
        .unwrap();
    let bob_accepted = server.submit_commit(add_bob.request).unwrap();
    let alice_page = server.sync_events(ROOM_ID, alice.device_ref(), 0).unwrap();
    alice
        .merge_pending_commit_from_log(ROOM_ID, &alice_page.entries, &add_bob.message_id)
        .unwrap();
    let welcome = server
        .claim_welcomes(bob.device_ref())
        .into_iter()
        .find(|welcome| welcome.welcome_id == "welcome_sync_bob")
        .unwrap();
    bob_store
        .activate_welcome_and_save(
            &mut bob,
            ROOM_ID,
            &welcome.welcome_payload,
            &welcome.ratchet_tree_payload,
            welcome.commit_seq,
        )
        .unwrap();
    server.ack_welcome("welcome_sync_bob", true).unwrap();
    let mut bob = bob_store.load_device(bob_config.clone()).unwrap();
    assert_eq!(bob.last_applied_seq(ROOM_ID).unwrap(), bob_accepted.seq);

    server
        .upload_key_package(
            charlie
                .upload_key_package_request("kp_sync_charlie")
                .unwrap(),
        )
        .unwrap();
    let claimed_charlie = server.claim_key_package("kp_sync_charlie").unwrap();
    let add_charlie = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_charlie,
            "welcome_sync_charlie",
            "add_sync_charlie",
        )
        .unwrap();
    let charlie_accepted = server.submit_commit(add_charlie.request).unwrap();
    let bob_page = server
        .sync_events(
            ROOM_ID,
            bob.device_ref(),
            bob.last_applied_seq(ROOM_ID).unwrap(),
        )
        .unwrap();
    assert_eq!(bob_page.entries.len(), 1);
    assert_eq!(
        bob_store
            .apply_log_entry_and_save(&mut bob, ROOM_ID, &bob_page.entries[0])
            .unwrap(),
        Some(AppliedLogEntry::Commit {
            sender: alice.device_ref().clone(),
            epoch: 2,
        })
    );

    let mut bob = bob_store.load_device(bob_config.clone()).unwrap();
    assert_eq!(bob.group_epoch(ROOM_ID).unwrap(), 2);
    assert_eq!(bob.last_applied_seq(ROOM_ID).unwrap(), charlie_accepted.seq);
    let replay_page = server
        .sync_events(ROOM_ID, bob.device_ref(), bob_accepted.seq)
        .unwrap();
    assert_eq!(replay_page.entries[0].seq, charlie_accepted.seq);
    assert_eq!(
        bob_store
            .apply_log_entry_and_save(&mut bob, ROOM_ID, &replay_page.entries[0])
            .unwrap(),
        None
    );

    let alice_page = server
        .sync_events(ROOM_ID, alice.device_ref(), bob_accepted.seq)
        .unwrap();
    alice
        .merge_pending_commit_from_log(ROOM_ID, &alice_page.entries, &add_charlie.message_id)
        .unwrap();
    let plaintext =
        br#"{"type":"finitecomputer.command.v1","body":{"text":"after commit resume"}}"#;
    let request = alice
        .create_application_request(ROOM_ID, plaintext, "sync_resume_message")
        .unwrap();
    let message_accepted = server.append_event(request).unwrap();
    let app_page = server
        .sync_events(
            ROOM_ID,
            bob.device_ref(),
            bob.last_applied_seq(ROOM_ID).unwrap(),
        )
        .unwrap();
    assert_eq!(app_page.entries.len(), 1);
    assert_eq!(
        bob_store
            .apply_log_entry_and_save(&mut bob, ROOM_ID, &app_page.entries[0])
            .unwrap(),
        Some(AppliedLogEntry::Application(plaintext.to_vec()))
    );

    let mut bob = bob_store.load_device(bob_config).unwrap();
    assert_eq!(bob.last_applied_seq(ROOM_ID).unwrap(), message_accepted.seq);
    let replay_app_page = server
        .sync_events(ROOM_ID, bob.device_ref(), charlie_accepted.seq)
        .unwrap();
    assert_eq!(replay_app_page.entries[0].seq, message_accepted.seq);
    assert_eq!(
        bob_store
            .apply_log_entry_and_save(&mut bob, ROOM_ID, &replay_app_page.entries[0])
            .unwrap(),
        None
    );
}

#[test]
fn client_processes_remote_add_commit_before_epoch_two_messages() {
    let (mut server, mut alice, mut bob, bob_join_seq) = active_alice_bob_room();
    let mut charlie = test_device(CHARLIE_ACCOUNT_SECRET_BYTES, "charlie_phone");

    server
        .upload_key_package(
            charlie
                .upload_key_package_request("kp_remote_charlie_1")
                .unwrap(),
        )
        .unwrap();
    let claimed_key_package = server.claim_key_package("kp_remote_charlie_1").unwrap();
    let prepared = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_key_package,
            "welcome_remote_charlie_1",
            "alice_remote_add_charlie",
        )
        .unwrap();
    let accepted = server.submit_commit(prepared.request).unwrap();
    let bob_page = server
        .sync_events(ROOM_ID, bob.device_ref(), bob_join_seq)
        .unwrap();
    assert_eq!(bob_page.entries.len(), 1);
    assert_eq!(bob_page.entries[0].kind, LogEntryKind::Commit);
    assert_eq!(
        bob.apply_log_entry(ROOM_ID, &bob_page.entries[0]).unwrap(),
        AppliedLogEntry::Commit {
            sender: alice.device_ref().clone(),
            epoch: 2,
        }
    );
    assert_eq!(bob.group_epoch(ROOM_ID).unwrap(), 2);

    let alice_page = server
        .sync_events(ROOM_ID, alice.device_ref(), bob_join_seq)
        .unwrap();
    assert_eq!(
        alice
            .apply_log_entry(ROOM_ID, &alice_page.entries[0])
            .unwrap(),
        AppliedLogEntry::Commit {
            sender: alice.device_ref().clone(),
            epoch: 2,
        }
    );
    assert_eq!(alice.group_epoch(ROOM_ID).unwrap(), 2);

    let charlie_join_seq =
        claim_and_activate(&mut server, &mut charlie, "welcome_remote_charlie_1");
    assert_eq!(charlie_join_seq, accepted.seq);
    assert_eq!(charlie.group_epoch(ROOM_ID).unwrap(), 2);

    let plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"remote add ok"}}"#;
    let request = bob
        .create_application_request(ROOM_ID, plaintext, "bob_after_remote_add")
        .unwrap();
    let app = server.append_event(request).unwrap();
    assert_eq!(app.seq, accepted.seq + 1);
    let alice_page = server
        .sync_events(ROOM_ID, alice.device_ref(), accepted.seq)
        .unwrap();
    let charlie_page = server
        .sync_events(ROOM_ID, charlie.device_ref(), accepted.seq)
        .unwrap();
    assert_eq!(
        alice
            .decrypt_application_entry(ROOM_ID, &alice_page.entries[0])
            .unwrap(),
        plaintext
    );
    assert_eq!(
        charlie
            .decrypt_application_entry(ROOM_ID, &charlie_page.entries[0])
            .unwrap(),
        plaintext
    );
}

#[test]
fn client_rejects_tampered_remote_commit_without_epoch_advance() {
    let (mut server, mut alice, mut bob, bob_join_seq) = active_alice_bob_room();
    let charlie = test_device(CHARLIE_ACCOUNT_SECRET_BYTES, "charlie_phone");

    server
        .upload_key_package(
            charlie
                .upload_key_package_request("kp_tampered_charlie_1")
                .unwrap(),
        )
        .unwrap();
    let claimed_key_package = server.claim_key_package("kp_tampered_charlie_1").unwrap();
    let prepared = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_key_package,
            "welcome_tampered_charlie_1",
            "alice_tampered_add_charlie",
        )
        .unwrap();
    server.submit_commit(prepared.request).unwrap();
    let bob_page = server
        .sync_events(ROOM_ID, bob.device_ref(), bob_join_seq)
        .unwrap();
    let mut tampered = bob_page.entries[0].clone();
    tampered.envelope.payload[0] ^= 0x01;

    let err = bob.apply_commit_entry(ROOM_ID, &tampered).unwrap_err();

    assert!(matches!(err, ClientError::LogEntryMessageIdMismatch { .. }));
    assert_eq!(bob.group_epoch(ROOM_ID).unwrap(), 1);
    assert!(!bob.has_pending_commit(ROOM_ID).unwrap());
}

#[test]
fn client_processes_remote_update_commit_before_epoch_three_messages() {
    let mut world = active_alice_bob_charlie_room();
    let alice_ref = world.alice.device_ref().clone();

    let prepared = world
        .alice
        .prepare_self_update_commit(ROOM_ID, "alice_rekey_epoch_2")
        .unwrap();
    let accepted = world.server.submit_commit(prepared.request).unwrap();
    assert_eq!(accepted.seq, world.last_seq + 1);
    assert_eq!(
        apply_one_commit(&world.server, &mut world.alice, world.last_seq),
        AppliedLogEntry::Commit {
            sender: alice_ref.clone(),
            epoch: 3,
        }
    );
    assert_eq!(
        apply_one_commit(&world.server, &mut world.bob, world.last_seq),
        AppliedLogEntry::Commit {
            sender: alice_ref.clone(),
            epoch: 3,
        }
    );
    assert_eq!(
        apply_one_commit(&world.server, &mut world.charlie, world.last_seq),
        AppliedLogEntry::Commit {
            sender: alice_ref,
            epoch: 3,
        }
    );
    assert_eq!(world.alice.group_epoch(ROOM_ID).unwrap(), 3);
    assert_eq!(world.bob.group_epoch(ROOM_ID).unwrap(), 3);
    assert_eq!(world.charlie.group_epoch(ROOM_ID).unwrap(), 3);

    let plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"remote update ok"}}"#;
    let request = world
        .bob
        .create_application_request(ROOM_ID, plaintext, "bob_after_remote_update")
        .unwrap();
    world.server.append_event(request).unwrap();
    assert_device_decrypts_after(&world.server, &mut world.alice, accepted.seq, plaintext);
    assert_device_decrypts_after(&world.server, &mut world.charlie, accepted.seq, plaintext);
}

#[test]
fn client_processes_remote_remove_commit_before_post_remove_messages() {
    let mut world = active_alice_bob_charlie_room();
    let bob_ref = world.bob.device_ref().clone();
    let charlie_ref = world.charlie.device_ref().clone();

    let prepared = world
        .bob
        .prepare_remove_member_commit(ROOM_ID, &charlie_ref, "bob_remove_charlie")
        .unwrap();
    let accepted = world.server.submit_commit(prepared.request).unwrap();
    assert_eq!(accepted.seq, world.last_seq + 1);
    assert_eq!(
        apply_one_commit(&world.server, &mut world.bob, world.last_seq),
        AppliedLogEntry::Commit {
            sender: bob_ref.clone(),
            epoch: 3,
        }
    );
    assert_eq!(
        apply_one_commit(&world.server, &mut world.alice, world.last_seq),
        AppliedLogEntry::Commit {
            sender: bob_ref.clone(),
            epoch: 3,
        }
    );
    assert_eq!(
        apply_one_commit(&world.server, &mut world.charlie, world.last_seq),
        AppliedLogEntry::Commit {
            sender: bob_ref,
            epoch: 3,
        }
    );
    assert!(matches!(
        world
            .charlie
            .create_application_request(ROOM_ID, b"removed", "charlie_removed_local"),
        Err(ClientError::CreateApplicationMessage)
    ));
    assert_eq!(
        world
            .server
            .append_event(fake_application_request(
                charlie_ref.clone(),
                3,
                "charlie_removed_server"
            ))
            .unwrap_err(),
        EngineError::SenderNotActive(charlie_ref.clone())
    );

    let plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"after remove"}}"#;
    let request = world
        .bob
        .create_application_request(ROOM_ID, plaintext, "bob_after_remove")
        .unwrap();
    world.server.append_event(request).unwrap();
    assert_device_decrypts_after(&world.server, &mut world.alice, accepted.seq, plaintext);
    let charlie_page = world
        .server
        .sync_events(ROOM_ID, &charlie_ref, accepted.seq)
        .unwrap();
    assert!(charlie_page.entries.is_empty());
}

#[test]
fn stale_removed_device_can_process_removal_but_not_future_ciphertext() {
    let mut world = active_alice_bob_charlie_room();
    let charlie_ref = world.charlie.device_ref().clone();
    let charlie_config = test_config(CHARLIE_ACCOUNT_SECRET_BYTES, "charlie_phone");
    let stale_charlie_state = world.charlie.export_state().unwrap();
    let mut stale_charlie =
        FiniteChatDevice::from_state(charlie_config, stale_charlie_state).unwrap();

    let prepared = world
        .bob
        .prepare_remove_member_commit(ROOM_ID, &charlie_ref, "bob_remove_stale_charlie")
        .unwrap();
    let accepted = world.server.submit_commit(prepared.request).unwrap();
    apply_one_commit(&world.server, &mut world.bob, world.last_seq);
    apply_one_commit(&world.server, &mut world.alice, world.last_seq);

    let stale_page = world
        .server
        .sync_events(ROOM_ID, stale_charlie.device_ref(), world.last_seq)
        .unwrap();
    assert_eq!(stale_page.entries.len(), 1);
    assert_eq!(stale_page.entries[0].seq, accepted.seq);
    let stale_send = stale_charlie
        .create_application_request(ROOM_ID, b"stale", "stale_charlie_old_epoch")
        .unwrap();
    assert!(matches!(
        world.server.append_event(stale_send).unwrap_err(),
        EngineError::WrongEpoch { .. }
    ));
    assert_eq!(
        world
            .server
            .append_event(fake_application_request(
                charlie_ref.clone(),
                3,
                "stale_charlie_fake_new_epoch"
            ))
            .unwrap_err(),
        EngineError::SenderNotActive(charlie_ref.clone())
    );
    assert_eq!(
        stale_charlie
            .apply_log_entry(ROOM_ID, &stale_page.entries[0])
            .unwrap(),
        AppliedLogEntry::Commit {
            sender: world.bob.device_ref().clone(),
            epoch: 3,
        }
    );
    assert!(matches!(
        stale_charlie.create_application_request(ROOM_ID, b"removed", "stale_charlie_removed"),
        Err(ClientError::CreateApplicationMessage)
    ));

    let plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"not for removed"}}"#;
    let request = world
        .bob
        .create_application_request(ROOM_ID, plaintext, "bob_after_stale_remove")
        .unwrap();
    world.server.append_event(request).unwrap();
    let alice_page = world
        .server
        .sync_events(ROOM_ID, world.alice.device_ref(), accepted.seq)
        .unwrap();
    assert_eq!(
        world
            .alice
            .decrypt_application_entry(ROOM_ID, &alice_page.entries[0])
            .unwrap(),
        plaintext
    );
    assert!(matches!(
        stale_charlie.decrypt_application_entry(ROOM_ID, &alice_page.entries[0]),
        Err(ClientError::ProcessMessage)
    ));
    let stale_future_page = world
        .server
        .sync_events(ROOM_ID, stale_charlie.device_ref(), accepted.seq)
        .unwrap();
    assert!(stale_future_page.entries.is_empty());
}

#[test]
fn client_recovers_losing_same_epoch_add_commit_and_retries() {
    let (mut server, mut alice, mut bob, bob_join_seq) = active_alice_bob_room();
    let mut charlie = test_device(CHARLIE_ACCOUNT_SECRET_BYTES, "charlie_phone");
    let mut dana = test_device(DANA_ACCOUNT_SECRET_BYTES, "dana_runtime");

    server
        .upload_key_package(
            charlie
                .upload_key_package_request("kp_race_charlie_1")
                .unwrap(),
        )
        .unwrap();
    server
        .upload_key_package(dana.upload_key_package_request("kp_race_dana_1").unwrap())
        .unwrap();
    let claimed_charlie = server.claim_key_package("kp_race_charlie_1").unwrap();
    let claimed_dana = server.claim_key_package("kp_race_dana_1").unwrap();

    let alice_winner = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_charlie,
            "welcome_race_charlie_1",
            "race_alice_add_charlie",
        )
        .unwrap();
    let bob_loser = bob
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_dana,
            "welcome_race_dana_1",
            "race_bob_add_dana_loses",
        )
        .unwrap();
    let alice_accepted = server.submit_commit(alice_winner.request).unwrap();
    assert!(matches!(
        server.submit_commit(bob_loser.request).unwrap_err(),
        EngineError::WrongEpoch { .. }
    ));
    assert!(server.welcome("welcome_race_dana_1").is_none());
    assert!(bob.has_pending_commit(ROOM_ID).unwrap());

    assert_eq!(
        apply_one_commit(&server, &mut bob, bob_join_seq),
        AppliedLogEntry::Commit {
            sender: alice.device_ref().clone(),
            epoch: 2,
        }
    );
    assert!(!bob.has_pending_commit(ROOM_ID).unwrap());
    assert_eq!(
        apply_one_commit(&server, &mut alice, bob_join_seq),
        AppliedLogEntry::Commit {
            sender: alice.device_ref().clone(),
            epoch: 2,
        }
    );
    let charlie_join_seq = claim_and_activate(&mut server, &mut charlie, "welcome_race_charlie_1");
    assert_eq!(charlie_join_seq, alice_accepted.seq);

    let bob_retry = bob
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_dana,
            "welcome_race_dana_1",
            "race_bob_add_dana_retry",
        )
        .unwrap();
    let dana_accepted = server.submit_commit(bob_retry.request).unwrap();
    assert_eq!(dana_accepted.seq, alice_accepted.seq + 1);
    assert_eq!(
        apply_one_commit(&server, &mut bob, alice_accepted.seq),
        AppliedLogEntry::Commit {
            sender: bob.device_ref().clone(),
            epoch: 3,
        }
    );
    assert_eq!(
        apply_one_commit(&server, &mut alice, alice_accepted.seq),
        AppliedLogEntry::Commit {
            sender: bob.device_ref().clone(),
            epoch: 3,
        }
    );
    assert_eq!(
        apply_one_commit(&server, &mut charlie, alice_accepted.seq),
        AppliedLogEntry::Commit {
            sender: bob.device_ref().clone(),
            epoch: 3,
        }
    );
    let dana_join_seq = claim_and_activate(&mut server, &mut dana, "welcome_race_dana_1");
    assert_eq!(dana_join_seq, dana_accepted.seq);

    let plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"race recovered"}}"#;
    let request = bob
        .create_application_request(ROOM_ID, plaintext, "bob_after_race_recovery")
        .unwrap();
    server.append_event(request).unwrap();
    assert_device_decrypts_after(&server, &mut alice, dana_accepted.seq, plaintext);
    assert_device_decrypts_after(&server, &mut charlie, dana_accepted.seq, plaintext);
    assert_device_decrypts_after(&server, &mut dana, dana_accepted.seq, plaintext);
}

#[test]
fn client_key_package_replenishment_edges_use_real_packages() {
    let alice_phone = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_phone_replenish");
    let mut server = DeliveryService::new();

    server
        .upload_key_package(
            alice_phone
                .upload_key_package_request("kp_replenish_1")
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        server
            .upload_key_package(
                alice_phone
                    .upload_key_package_request("kp_replenish_1")
                    .unwrap(),
            )
            .unwrap_err(),
        EngineError::KeyPackageAlreadyExists("kp_replenish_1".to_string())
    );
    let first_claim = server
        .claim_key_packages_for_account(&alice_phone.device_ref().account_id)
        .unwrap();
    assert_eq!(first_claim.len(), 1);
    assert_eq!(first_claim[0].key_package_id, "kp_replenish_1");
    assert_eq!(
        server.key_package("kp_replenish_1").unwrap().state,
        KeyPackageState::Leased
    );
    assert!(
        server
            .claim_key_packages_for_account(&alice_phone.device_ref().account_id)
            .unwrap()
            .is_empty()
    );

    server
        .upload_key_package(
            alice_phone
                .upload_key_package_request("kp_replenish_2")
                .unwrap(),
        )
        .unwrap();
    let replenished_claim = server
        .claim_key_packages_for_account(&alice_phone.device_ref().account_id)
        .unwrap();
    assert_eq!(replenished_claim.len(), 1);
    assert_eq!(replenished_claim[0].key_package_id, "kp_replenish_2");
    assert_eq!(
        server.key_package("kp_replenish_2").unwrap().state,
        KeyPackageState::Leased
    );

    server.expire_key_package_lease("kp_replenish_1").unwrap();
    assert_eq!(
        server.key_package("kp_replenish_1").unwrap().state,
        KeyPackageState::Available
    );
    let reclaimed = server.claim_key_package("kp_replenish_1").unwrap();
    assert_eq!(reclaimed.key_package_id, "kp_replenish_1");
    assert_eq!(
        server.key_package("kp_replenish_1").unwrap().state,
        KeyPackageState::Leased
    );
}

#[test]
fn new_device_history_policy_starts_at_add_commit_not_prior_messages() {
    let room_id = "room_history_policy";
    let mls_group_id = "mls_history_policy";
    let mut bob = test_device(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let mut alice_phone = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_phone_history");
    let mut server = DeliveryService::new();

    server
        .create_room(CreateRoomRequest {
            room_id: room_id.to_string(),
            mls_group_id: mls_group_id.to_string(),
            creator: bob.device_ref().clone(),
        })
        .unwrap();
    bob.create_group_state(room_id, mls_group_id).unwrap();
    let prior_plaintext =
        br#"{"type":"finitecomputer.command.v1","body":{"text":"before invite"}}"#;
    let prior_request = bob
        .create_application_request(room_id, prior_plaintext, "history_before_invite")
        .unwrap();
    let prior = server.append_event(prior_request).unwrap();

    server
        .upload_key_package(
            alice_phone
                .upload_key_package_request("kp_history_alice_phone")
                .unwrap(),
        )
        .unwrap();
    let claimed_key_package = server.claim_key_package("kp_history_alice_phone").unwrap();
    let prepared = bob
        .prepare_add_member_commit(
            room_id,
            &claimed_key_package,
            "welcome_history_alice_phone",
            "history_add_alice_phone",
        )
        .unwrap();
    let accepted = server.submit_commit(prepared.request).unwrap();
    assert_eq!(
        apply_one_commit_for_room(&server, room_id, &mut bob, prior.seq),
        AppliedLogEntry::Commit {
            sender: bob.device_ref().clone(),
            epoch: 1,
        }
    );
    let join_seq = claim_and_activate_room(
        &mut server,
        &mut alice_phone,
        room_id,
        "welcome_history_alice_phone",
    );
    assert_eq!(join_seq, accepted.seq);

    let post_plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"after invite"}}"#;
    let post_request = bob
        .create_application_request(room_id, post_plaintext, "history_after_invite")
        .unwrap();
    server.append_event(post_request).unwrap();

    let full_page = server
        .sync_events(room_id, alice_phone.device_ref(), 0)
        .unwrap();
    assert!(
        full_page
            .entries
            .iter()
            .all(|entry| entry.seq >= accepted.seq)
    );
    assert!(
        !full_page
            .entries
            .iter()
            .any(|entry| entry.message_id == prior.message_id)
    );
    assert_eq!(full_page.entries[0].kind, LogEntryKind::Commit);
    assert_eq!(full_page.entries[0].seq, accepted.seq);
    assert_eq!(full_page.entries[1].kind, LogEntryKind::Application);
    assert_eq!(
        alice_phone
            .decrypt_application_entry(room_id, &full_page.entries[1])
            .unwrap(),
        post_plaintext
    );
}

#[test]
fn client_links_new_device_into_existing_rooms_with_distinct_key_packages() {
    let mut server = DeliveryService::new();
    let mut alice_browser = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser");
    let mut alice_phone = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_phone_late");
    let mut bob = test_device(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let mut dana = test_device(DANA_ACCOUNT_SECRET_BYTES, "dana_runtime");
    let room_a = "room_late_link_a";
    let group_a = "mls_late_link_a";
    let room_b = "room_late_link_b";
    let group_b = "mls_late_link_b";

    let bob_join_seq = create_group_room_with_member(
        &mut server,
        &mut alice_browser,
        &mut bob,
        GroupMemberSetup {
            room_id: room_a,
            mls_group_id: group_a,
            key_package_id: "kp_bob_late_link_a",
            welcome_id: "welcome_bob_late_link_a",
            idempotency_key: "add_bob_late_link_a",
        },
    );
    let dana_join_seq = create_group_room_with_member(
        &mut server,
        &mut alice_browser,
        &mut dana,
        GroupMemberSetup {
            room_id: room_b,
            mls_group_id: group_b,
            key_package_id: "kp_dana_late_link_b",
            welcome_id: "welcome_dana_late_link_b",
            idempotency_key: "add_dana_late_link_b",
        },
    );

    server
        .upload_key_package(
            alice_phone
                .upload_key_package_request("kp_alice_phone_late_a")
                .unwrap(),
        )
        .unwrap();
    server
        .upload_key_package(
            alice_phone
                .upload_key_package_request("kp_alice_phone_late_b")
                .unwrap(),
        )
        .unwrap();

    let phone_claim_a = server.claim_key_package("kp_alice_phone_late_a").unwrap();
    let prepared_a = alice_browser
        .prepare_add_member_commit(
            room_a,
            &phone_claim_a,
            "welcome_alice_phone_late_a",
            "link_alice_phone_room_a",
        )
        .unwrap();
    let accepted_a = server.submit_commit(prepared_a.request).unwrap();
    assert_eq!(
        apply_one_commit_for_room(&server, room_a, &mut alice_browser, bob_join_seq),
        AppliedLogEntry::Commit {
            sender: alice_browser.device_ref().clone(),
            epoch: 2,
        }
    );
    assert_eq!(
        apply_one_commit_for_room(&server, room_a, &mut bob, bob_join_seq),
        AppliedLogEntry::Commit {
            sender: alice_browser.device_ref().clone(),
            epoch: 2,
        }
    );
    let phone_join_a = claim_and_activate_room(
        &mut server,
        &mut alice_phone,
        room_a,
        "welcome_alice_phone_late_a",
    );
    assert_eq!(phone_join_a, accepted_a.seq);

    let phone_claim_b = server.claim_key_package("kp_alice_phone_late_b").unwrap();
    let prepared_b = alice_browser
        .prepare_add_member_commit(
            room_b,
            &phone_claim_b,
            "welcome_alice_phone_late_b",
            "link_alice_phone_room_b",
        )
        .unwrap();
    let accepted_b = server.submit_commit(prepared_b.request).unwrap();
    assert_eq!(
        apply_one_commit_for_room(&server, room_b, &mut alice_browser, dana_join_seq),
        AppliedLogEntry::Commit {
            sender: alice_browser.device_ref().clone(),
            epoch: 2,
        }
    );
    assert_eq!(
        apply_one_commit_for_room(&server, room_b, &mut dana, dana_join_seq),
        AppliedLogEntry::Commit {
            sender: alice_browser.device_ref().clone(),
            epoch: 2,
        }
    );
    let phone_join_b = claim_and_activate_room(
        &mut server,
        &mut alice_phone,
        room_b,
        "welcome_alice_phone_late_b",
    );
    assert_eq!(phone_join_b, accepted_b.seq);
    assert_eq!(alice_phone.group_epoch(room_a).unwrap(), 2);
    assert_eq!(alice_phone.group_epoch(room_b).unwrap(), 2);

    let room_a_plaintext =
        br#"{"type":"finitecomputer.command.v1","body":{"text":"room a after link"}}"#;
    let room_a_request = bob
        .create_application_request(room_a, room_a_plaintext, "bob_after_late_link")
        .unwrap();
    server.append_event(room_a_request).unwrap();
    assert_device_decrypts_after_for_room(
        &server,
        room_a,
        &mut alice_phone,
        accepted_a.seq,
        room_a_plaintext,
    );

    let room_b_plaintext =
        br#"{"type":"finitecomputer.command.v1","body":{"text":"room b after link"}}"#;
    let room_b_request = dana
        .create_application_request(room_b, room_b_plaintext, "dana_after_late_link")
        .unwrap();
    server.append_event(room_b_request).unwrap();
    assert_device_decrypts_after_for_room(
        &server,
        room_b,
        &mut alice_phone,
        accepted_b.seq,
        room_b_plaintext,
    );
}

#[test]
fn multi_device_real_mls_ordering_matrix_validates_late_catch_up() {
    let activation_orders = [
        ["alice_browser", "alice_phone", "alice_tablet"],
        ["alice_browser", "alice_tablet", "alice_phone"],
        ["alice_phone", "alice_browser", "alice_tablet"],
        ["alice_phone", "alice_tablet", "alice_browser"],
        ["alice_tablet", "alice_browser", "alice_phone"],
        ["alice_tablet", "alice_phone", "alice_browser"],
    ];
    let message_patterns = [[2, 1, 1, 1], [0, 3, 1, 2], [4, 0, 2, 1], [1, 2, 0, 3]];

    let mut scenario_index = 0usize;
    for activation_order in activation_orders {
        for message_pattern in message_patterns {
            scenario_index += 1;
            run_real_mls_multi_device_ordering_scenario(
                scenario_index,
                activation_order,
                message_pattern,
            );
        }
    }
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
    FiniteChatDevice::new(test_config(account_secret_bytes, device_id)).unwrap()
}

fn test_config(
    account_secret_bytes: [u8; NOSTR_SECRET_KEY_BYTES],
    device_id: &str,
) -> FiniteChatDeviceConfig {
    FiniteChatDeviceConfig {
        account_secret_key: NostrSecretKey::from_bytes(account_secret_bytes).unwrap(),
        device_id: device_id.to_string(),
        now_unix_seconds: NOW,
        credential_not_before_unix_seconds: NOW - 60,
        credential_not_after_unix_seconds: NOW + 60,
    }
}

fn sqlite_client_store(
    path: impl AsRef<std::path::Path>,
    config: &FiniteChatDeviceConfig,
) -> SqliteClientStore {
    SqliteClientStore::open(
        path,
        SqliteClientStoreOptions::from_nostr_secret(&config.account_secret_key, &config.device_id)
            .unwrap(),
    )
    .unwrap()
}

fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn hex_lower(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(TABLE[(byte >> 4) as usize] as char);
        out.push(TABLE[(byte & 0x0f) as usize] as char);
    }
    out
}

fn welcome_id_for(
    claims: &[finitechat_engine::ClaimKeyPackageResult],
    welcome_ids: &[String],
    device_id: &str,
) -> String {
    welcome_ids
        .iter()
        .zip(claims)
        .find(|(_, claim)| claim.owner.device_id == device_id)
        .map(|(welcome_id, _)| welcome_id.clone())
        .unwrap()
}

fn claim_and_activate(
    server: &mut DeliveryService,
    device: &mut FiniteChatDevice,
    welcome_id: &str,
) -> u64 {
    claim_and_activate_room(server, device, ROOM_ID, welcome_id)
}

fn claim_and_activate_room(
    server: &mut DeliveryService,
    device: &mut FiniteChatDevice,
    room_id: &str,
    welcome_id: &str,
) -> u64 {
    let claimed_welcomes = server.claim_welcomes(device.device_ref());
    let welcome = claimed_welcomes
        .into_iter()
        .find(|welcome| welcome.welcome_id == welcome_id)
        .unwrap();
    device
        .activate_welcome(
            room_id,
            &welcome.welcome_payload,
            &welcome.ratchet_tree_payload,
        )
        .unwrap();
    server.ack_welcome(welcome_id, true).unwrap();
    welcome.commit_seq
}

fn active_alice_bob_room() -> (DeliveryService, FiniteChatDevice, FiniteChatDevice, u64) {
    let mut alice = test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser");
    let mut bob = test_device(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let mut server = DeliveryService::new();

    server
        .create_room(CreateRoomRequest {
            room_id: ROOM_ID.to_string(),
            mls_group_id: MLS_GROUP_ID.to_string(),
            creator: alice.device_ref().clone(),
        })
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
            "activate_bob_helper",
        )
        .unwrap();
    let accepted = server.submit_commit(prepared.request).unwrap();
    let alice_page = server.sync_events(ROOM_ID, alice.device_ref(), 0).unwrap();
    alice
        .merge_pending_commit_from_log(ROOM_ID, &alice_page.entries, &prepared.message_id)
        .unwrap();
    let bob_join_seq = claim_and_activate(&mut server, &mut bob, BOB_WELCOME_ID);
    assert_eq!(bob_join_seq, accepted.seq);
    assert_eq!(alice.group_epoch(ROOM_ID).unwrap(), 1);
    assert_eq!(bob.group_epoch(ROOM_ID).unwrap(), 1);
    (server, alice, bob, bob_join_seq)
}

struct GroupMemberSetup<'a> {
    room_id: &'a str,
    mls_group_id: &'a str,
    key_package_id: &'a str,
    welcome_id: &'a str,
    idempotency_key: &'a str,
}

fn create_group_room_with_member(
    server: &mut DeliveryService,
    alice: &mut FiniteChatDevice,
    member: &mut FiniteChatDevice,
    setup: GroupMemberSetup<'_>,
) -> u64 {
    server
        .create_room(CreateRoomRequest {
            room_id: setup.room_id.to_string(),
            mls_group_id: setup.mls_group_id.to_string(),
            creator: alice.device_ref().clone(),
        })
        .unwrap();
    alice
        .create_group_state(setup.room_id, setup.mls_group_id)
        .unwrap();
    server
        .upload_key_package(
            member
                .upload_key_package_request(setup.key_package_id)
                .unwrap(),
        )
        .unwrap();
    let claimed_key_package = server.claim_key_package(setup.key_package_id).unwrap();
    let prepared = alice
        .prepare_add_member_commit(
            setup.room_id,
            &claimed_key_package,
            setup.welcome_id,
            setup.idempotency_key,
        )
        .unwrap();
    let accepted = server.submit_commit(prepared.request).unwrap();
    let alice_page = server
        .sync_events(setup.room_id, alice.device_ref(), 0)
        .unwrap();
    alice
        .merge_pending_commit_from_log(setup.room_id, &alice_page.entries, &prepared.message_id)
        .unwrap();
    let join_seq = claim_and_activate_room(server, member, setup.room_id, setup.welcome_id);
    assert_eq!(join_seq, accepted.seq);
    assert_eq!(alice.group_epoch(setup.room_id).unwrap(), 1);
    assert_eq!(member.group_epoch(setup.room_id).unwrap(), 1);
    join_seq
}

struct ActiveThreeMemberRoom {
    server: DeliveryService,
    alice: FiniteChatDevice,
    bob: FiniteChatDevice,
    charlie: FiniteChatDevice,
    last_seq: u64,
}

fn active_alice_bob_charlie_room() -> ActiveThreeMemberRoom {
    let (mut server, mut alice, mut bob, bob_join_seq) = active_alice_bob_room();
    let mut charlie = test_device(CHARLIE_ACCOUNT_SECRET_BYTES, "charlie_phone");

    server
        .upload_key_package(
            charlie
                .upload_key_package_request("kp_active_charlie_1")
                .unwrap(),
        )
        .unwrap();
    let claimed_key_package = server.claim_key_package("kp_active_charlie_1").unwrap();
    let prepared = alice
        .prepare_add_member_commit(
            ROOM_ID,
            &claimed_key_package,
            "welcome_active_charlie_1",
            "alice_add_active_charlie",
        )
        .unwrap();
    let accepted = server.submit_commit(prepared.request).unwrap();
    assert_eq!(
        apply_one_commit(&server, &mut alice, bob_join_seq),
        AppliedLogEntry::Commit {
            sender: alice.device_ref().clone(),
            epoch: 2,
        }
    );
    assert_eq!(
        apply_one_commit(&server, &mut bob, bob_join_seq),
        AppliedLogEntry::Commit {
            sender: alice.device_ref().clone(),
            epoch: 2,
        }
    );
    let charlie_join_seq =
        claim_and_activate(&mut server, &mut charlie, "welcome_active_charlie_1");
    assert_eq!(charlie_join_seq, accepted.seq);
    assert_eq!(alice.group_epoch(ROOM_ID).unwrap(), 2);
    assert_eq!(bob.group_epoch(ROOM_ID).unwrap(), 2);
    assert_eq!(charlie.group_epoch(ROOM_ID).unwrap(), 2);

    ActiveThreeMemberRoom {
        server,
        alice,
        bob,
        charlie,
        last_seq: accepted.seq,
    }
}

fn apply_one_commit(
    server: &DeliveryService,
    device: &mut FiniteChatDevice,
    after_seq: u64,
) -> AppliedLogEntry {
    apply_one_commit_for_room(server, ROOM_ID, device, after_seq)
}

fn apply_one_commit_for_room(
    server: &DeliveryService,
    room_id: &str,
    device: &mut FiniteChatDevice,
    after_seq: u64,
) -> AppliedLogEntry {
    let page = server
        .sync_events(room_id, device.device_ref(), after_seq)
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].kind, LogEntryKind::Commit);
    device.apply_log_entry(room_id, &page.entries[0]).unwrap()
}

fn assert_device_decrypts_after(
    server: &DeliveryService,
    device: &mut FiniteChatDevice,
    after_seq: u64,
    plaintext: &[u8],
) {
    assert_device_decrypts_after_for_room(server, ROOM_ID, device, after_seq, plaintext);
}

fn assert_device_decrypts_after_for_room(
    server: &DeliveryService,
    room_id: &str,
    device: &mut FiniteChatDevice,
    after_seq: u64,
    plaintext: &[u8],
) {
    let page = server
        .sync_events(room_id, device.device_ref(), after_seq)
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(
        device
            .decrypt_application_entry(room_id, &page.entries[0])
            .unwrap(),
        plaintext
    );
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

struct ScenarioAliceDevice {
    device: FiniteChatDevice,
    welcome_id: String,
    cursor: u64,
    decrypted_count: usize,
}

#[derive(Debug)]
struct SentPlaintext {
    seq: u64,
    plaintext: Vec<u8>,
}

fn run_real_mls_multi_device_ordering_scenario(
    scenario_index: usize,
    activation_order: [&str; 3],
    message_pattern: [usize; 4],
) {
    let mut bob = test_device(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let mut server = DeliveryService::new();
    let mut alice_devices = vec![
        ScenarioAliceDevice {
            device: test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser"),
            welcome_id: String::new(),
            cursor: 0,
            decrypted_count: 0,
        },
        ScenarioAliceDevice {
            device: test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_phone"),
            welcome_id: String::new(),
            cursor: 0,
            decrypted_count: 0,
        },
        ScenarioAliceDevice {
            device: test_device(ALICE_ACCOUNT_SECRET_BYTES, "alice_tablet"),
            welcome_id: String::new(),
            cursor: 0,
            decrypted_count: 0,
        },
    ];

    server
        .create_or_get_direct_room(bob.create_direct_room_request(
            ROOM_ID,
            MLS_GROUP_ID,
            alice_devices[0].device.device_ref().account_id.clone(),
        ))
        .unwrap();
    bob.create_group_state(ROOM_ID, MLS_GROUP_ID).unwrap();
    for alice_device in &alice_devices {
        server
            .upload_key_package(
                alice_device
                    .device
                    .upload_key_package_request(format!(
                        "kp_{}_{}",
                        alice_device.device.device_ref().device_id,
                        scenario_index
                    ))
                    .unwrap(),
            )
            .unwrap();
    }

    let claimed_key_packages = server
        .claim_key_packages_for_account(&alice_devices[0].device.device_ref().account_id)
        .unwrap();
    assert_eq!(claimed_key_packages.len(), alice_devices.len());
    let welcome_ids = claimed_key_packages
        .iter()
        .map(|claim| format!("welcome_{}_{}", claim.owner.device_id, scenario_index))
        .collect::<Vec<_>>();
    for alice_device in &mut alice_devices {
        alice_device.welcome_id = welcome_ids
            .iter()
            .zip(&claimed_key_packages)
            .find(|(_, claim)| claim.owner == *alice_device.device.device_ref())
            .map(|(welcome_id, _)| welcome_id.clone())
            .unwrap();
    }

    let prepared = bob
        .prepare_add_members_commit(
            ROOM_ID,
            &claimed_key_packages,
            &welcome_ids,
            format!("invite_all_alice_devices_{scenario_index}"),
        )
        .unwrap();
    let accepted = server.submit_commit(prepared.request).unwrap();
    let bob_page = server.sync_events(ROOM_ID, bob.device_ref(), 0).unwrap();
    bob.merge_pending_commit_from_log(ROOM_ID, &bob_page.entries, &prepared.message_id)
        .unwrap();
    for alice_device in &mut alice_devices {
        alice_device.cursor = accepted.seq;
    }

    let mut sent_plaintexts = Vec::new();
    let mut next_message_index = 0usize;
    send_bob_messages(
        &mut server,
        &mut bob,
        scenario_index,
        &mut next_message_index,
        message_pattern[0],
        &mut sent_plaintexts,
    );
    assert_pending_devices_can_sync_but_not_send(&mut server, &alice_devices, &sent_plaintexts);

    for activation_step in 0..activation_order.len() {
        let device_index = alice_devices
            .iter()
            .position(|alice_device| {
                alice_device.device.device_ref().device_id == activation_order[activation_step]
            })
            .unwrap();
        let alice_device = &mut alice_devices[device_index];
        let join_seq = claim_and_activate(
            &mut server,
            &mut alice_device.device,
            &alice_device.welcome_id,
        );
        assert_eq!(join_seq, accepted.seq);
        drain_device_messages(&server, alice_device, &sent_plaintexts);

        send_bob_messages(
            &mut server,
            &mut bob,
            scenario_index,
            &mut next_message_index,
            message_pattern[activation_step + 1],
            &mut sent_plaintexts,
        );
        for alice_device in alice_devices.iter_mut().filter(|alice_device| {
            alice_device
                .device
                .group_epoch(ROOM_ID)
                .is_ok_and(|epoch| epoch == 1)
        }) {
            drain_device_messages(&server, alice_device, &sent_plaintexts);
        }
        assert_pending_devices_can_sync_but_not_send(&mut server, &alice_devices, &sent_plaintexts);
    }

    assert_eq!(
        server.room(ROOM_ID).unwrap().last_seq,
        1 + sent_plaintexts.len() as u64
    );
    for alice_device in &mut alice_devices {
        drain_device_messages(&server, alice_device, &sent_plaintexts);
        assert_eq!(alice_device.decrypted_count, sent_plaintexts.len());
        assert_eq!(alice_device.cursor, server.room(ROOM_ID).unwrap().last_seq);
    }
}

fn send_bob_messages(
    server: &mut DeliveryService,
    bob: &mut FiniteChatDevice,
    scenario_index: usize,
    next_message_index: &mut usize,
    count: usize,
    sent_plaintexts: &mut Vec<SentPlaintext>,
) {
    for _ in 0..count {
        *next_message_index += 1;
        let plaintext = format!(
            r#"{{"type":"finitecomputer.command.v1","body":{{"scenario":{scenario_index},"message":{}}}}}"#,
            *next_message_index
        )
        .into_bytes();
        let request = bob
            .create_application_request(
                ROOM_ID,
                &plaintext,
                format!("bob_msg_{scenario_index}_{}", *next_message_index),
            )
            .unwrap();
        let accepted = server.append_event(request).unwrap();
        assert_application_acceptance(&accepted, sent_plaintexts);
        sent_plaintexts.push(SentPlaintext {
            seq: accepted.seq,
            plaintext,
        });
    }
}

fn assert_application_acceptance(accepted: &EventAccepted, sent_plaintexts: &[SentPlaintext]) {
    let expected_seq = sent_plaintexts
        .last()
        .map(|message| message.seq + 1)
        .unwrap_or(2);
    assert_eq!(accepted.seq, expected_seq);
}

fn drain_device_messages(
    server: &DeliveryService,
    alice_device: &mut ScenarioAliceDevice,
    sent_plaintexts: &[SentPlaintext],
) {
    let page = server
        .sync_events(
            ROOM_ID,
            alice_device.device.device_ref(),
            alice_device.cursor,
        )
        .unwrap();
    assert!(!page.has_more);
    for entry in &page.entries {
        assert_eq!(entry.kind, LogEntryKind::Application);
        let expected = sent_plaintexts
            .iter()
            .find(|message| message.seq == entry.seq)
            .unwrap();
        let decrypted = alice_device
            .device
            .decrypt_application_entry(ROOM_ID, entry)
            .unwrap();
        assert_eq!(decrypted, expected.plaintext);
        alice_device.cursor = entry.seq;
        alice_device.decrypted_count += 1;
    }
    assert_eq!(
        alice_device.decrypted_count,
        count_messages_at_or_before(sent_plaintexts, alice_device.cursor)
    );
}

fn count_messages_at_or_before(sent_plaintexts: &[SentPlaintext], cursor: u64) -> usize {
    sent_plaintexts
        .iter()
        .filter(|message| message.seq <= cursor)
        .count()
}

fn assert_pending_devices_can_sync_but_not_send(
    server: &mut DeliveryService,
    alice_devices: &[ScenarioAliceDevice],
    sent_plaintexts: &[SentPlaintext],
) {
    for alice_device in alice_devices {
        if alice_device.device.group_epoch(ROOM_ID).is_ok() {
            continue;
        }
        let page = server
            .sync_events(
                ROOM_ID,
                alice_device.device.device_ref(),
                alice_device.cursor,
            )
            .unwrap();
        assert_eq!(
            page.entries.len(),
            sent_plaintexts.len() - alice_device.decrypted_count
        );
        assert_eq!(
            server
                .append_event(fake_application_request(
                    alice_device.device.device_ref().clone(),
                    1,
                    &format!(
                        "pending_send_{}_{}",
                        alice_device.device.device_ref().device_id,
                        sent_plaintexts.len()
                    )
                ))
                .unwrap_err(),
            EngineError::SenderNotActive(alice_device.device.device_ref().clone())
        );
    }
}
