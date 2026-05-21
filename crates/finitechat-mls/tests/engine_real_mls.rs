use finitechat_engine::{
    AppendEventRequest, CreateDirectRoomRequest, DeliveryService, SubmitCommitRequest,
    UploadKeyPackageRequest, device, envelope,
};
use finitechat_mls::{
    ExpectedDeviceCredential, FiniteDeviceCredentialV1, NOSTR_SECRET_KEY_BYTES, NostrSecretKey,
};
use finitechat_proto::{
    DeviceRef, LogEntryKind, MembershipAddV1, MembershipDeltaV1, StagedWelcomeV1, WelcomeState,
    message_id_for_bytes,
};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::{
    Ciphersuite, CredentialWithKey, GroupId, KeyPackage, KeyPackageBundle, MlsGroup,
    MlsGroupCreateConfig, MlsMessageBodyIn, MlsMessageIn, MlsMessageOut, OpenMlsProvider,
    ProcessedMessageContent, ProtocolMessage, RatchetTreeIn, StagedWelcome, Welcome,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;

const ALICE_ACCOUNT_SECRET_BYTES: [u8; NOSTR_SECRET_KEY_BYTES] = [17; NOSTR_SECRET_KEY_BYTES];
const BOB_ACCOUNT_SECRET_BYTES: [u8; NOSTR_SECRET_KEY_BYTES] = [19; NOSTR_SECRET_KEY_BYTES];
const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
const ROOM_ID: &str = "room_real_mls_direct";
const MLS_GROUP_ID: &str = "mls_real_mls_direct";
const BOB_KEY_PACKAGE_ID: &str = "kp_bob_real_mls_1";
const BOB_WELCOME_ID: &str = "welcome_bob_real_mls_1";
const NOW: u64 = 1_800_000_000;

#[test]
fn real_openmls_bytes_flow_through_engine_ordering() {
    let alice = TestMlsDevice::new(ALICE_ACCOUNT_SECRET_BYTES, "alice_browser");
    let bob = TestMlsDevice::new(BOB_ACCOUNT_SECRET_BYTES, "bob_runtime");
    let bob_key_package = bob.key_package_bundle();
    let bob_key_package_metadata = KeyPackageMetadata::from_bundle(&bob, &bob_key_package);

    let mut server = DeliveryService::new();
    server
        .create_or_get_direct_room(CreateDirectRoomRequest {
            room_id: ROOM_ID.to_string(),
            mls_group_id: MLS_GROUP_ID.to_string(),
            creator: alice.device_ref.clone(),
            other_account_id: bob.device_ref.account_id.clone(),
        })
        .unwrap();
    server
        .upload_key_package(UploadKeyPackageRequest {
            key_package_id: BOB_KEY_PACKAGE_ID.to_string(),
            owner: bob.device_ref.clone(),
            key_package_ref: bob_key_package_metadata.key_package_ref.clone(),
            key_package_hash: bob_key_package_metadata.key_package_hash.clone(),
        })
        .unwrap();
    server.claim_key_package(BOB_KEY_PACKAGE_ID).unwrap();

    let group_config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(false)
        .build();
    let group_id = GroupId::from_slice(MLS_GROUP_ID.as_bytes());
    let mut alice_group = MlsGroup::new_with_group_id(
        &alice.provider,
        &alice.signer,
        &group_config,
        group_id,
        alice.credential_with_key.clone(),
    )
    .unwrap();

    let (commit_message, welcome_message, _group_info) = alice_group
        .add_members(
            &alice.provider,
            &alice.signer,
            &[bob_key_package.key_package().clone()],
        )
        .unwrap();
    let commit_payload = mls_message_out_bytes(commit_message);
    let welcome_payload = mls_message_out_bytes(welcome_message);
    let ratchet_tree: RatchetTreeIn = alice_group
        .pending_commit()
        .expect("add_members should stage a pending commit")
        .export_ratchet_tree(alice.provider.crypto(), alice_group.export_ratchet_tree())
        .expect("pending commit should export ratchet tree")
        .expect("member commit should have a staged ratchet tree")
        .into();
    let ratchet_tree_payload = ratchet_tree
        .tls_serialize_detached()
        .expect("ratchet tree should serialize");

    assert_eq!(alice_group.epoch().as_u64(), 0);
    assert_eq!(alice_group.members().count(), 1);
    assert!(alice_group.pending_commit().is_some());

    let commit_envelope = envelope(
        ROOM_ID.to_string(),
        MLS_GROUP_ID.to_string(),
        alice.device_ref.clone(),
        0,
        LogEntryKind::Commit,
        commit_payload.clone(),
    );
    let commit_message_id = commit_envelope.message_id().unwrap();
    let accepted_commit = server
        .submit_commit(SubmitCommitRequest {
            room_id: ROOM_ID.to_string(),
            sender: alice.device_ref.clone(),
            expected_epoch: 0,
            envelope: commit_envelope,
            membership_delta: MembershipDeltaV1 {
                base_epoch: 0,
                post_commit_epoch: 1,
                commit_message_id: commit_message_id.clone(),
                adds: vec![MembershipAddV1 {
                    device: bob.device_ref.clone(),
                    key_package_id: BOB_KEY_PACKAGE_ID.to_string(),
                    key_package_ref: bob_key_package_metadata.key_package_ref,
                    key_package_hash: bob_key_package_metadata.key_package_hash,
                    welcome_id: BOB_WELCOME_ID.to_string(),
                }],
                removes: vec![],
            },
            idempotency_key: "real_mls_add_bob".to_string(),
            staged_welcomes: vec![StagedWelcomeV1 {
                welcome_id: BOB_WELCOME_ID.to_string(),
                welcome_payload: welcome_payload.clone(),
                ratchet_tree_payload: ratchet_tree_payload.clone(),
            }],
        })
        .unwrap();

    assert_eq!(accepted_commit.seq, 1);
    assert_eq!(accepted_commit.message_id, commit_message_id);
    assert_eq!(accepted_commit.released_welcomes, vec![BOB_WELCOME_ID]);
    assert_eq!(
        server.welcome(BOB_WELCOME_ID).unwrap().state,
        WelcomeState::Released
    );
    assert_eq!(alice_group.epoch().as_u64(), 0);
    assert!(alice_group.pending_commit().is_some());

    let alice_page = server.sync_events(ROOM_ID, &alice.device_ref, 0).unwrap();
    assert_eq!(alice_page.entries.len(), 1);
    assert_eq!(alice_page.entries[0].seq, accepted_commit.seq);
    assert_eq!(alice_page.entries[0].message_id, accepted_commit.message_id);
    assert_eq!(alice_page.entries[0].envelope.payload, commit_payload);

    alice_group.merge_pending_commit(&alice.provider).unwrap();
    assert_eq!(alice_group.epoch().as_u64(), 1);
    assert_eq!(alice_group.members().count(), 2);
    assert_eq!(
        server.welcome(BOB_WELCOME_ID).unwrap().welcome_payload,
        welcome_payload
    );
    assert_eq!(
        server.welcome(BOB_WELCOME_ID).unwrap().ratchet_tree_payload,
        ratchet_tree_payload
    );

    let claimed_welcomes = server.claim_welcomes(&bob.device_ref);
    assert_eq!(claimed_welcomes.len(), 1);
    assert_eq!(claimed_welcomes[0].welcome_id, BOB_WELCOME_ID);
    assert_eq!(claimed_welcomes[0].welcome_payload, welcome_payload);
    assert_eq!(
        claimed_welcomes[0].ratchet_tree_payload,
        ratchet_tree_payload
    );

    let mut bob_group = StagedWelcome::new_from_welcome(
        &bob.provider,
        group_config.join_config(),
        welcome_from_bytes(&claimed_welcomes[0].welcome_payload),
        Some(ratchet_tree_from_bytes(
            &claimed_welcomes[0].ratchet_tree_payload,
        )),
    )
    .unwrap()
    .into_group(&bob.provider)
    .unwrap();
    assert_eq!(bob_group.epoch().as_u64(), 1);
    assert_eq!(bob_group.group_id(), alice_group.group_id());
    assert_verified_member(&bob_group, &alice);
    assert_verified_member(&bob_group, &bob);

    server.ack_welcome(BOB_WELCOME_ID, true).unwrap();
    assert_eq!(
        server.welcome(BOB_WELCOME_ID).unwrap().state,
        WelcomeState::Acked
    );

    let plaintext = br#"{"type":"finitecomputer.command.v1","body":{"text":"run tests"}}"#;
    let app_message = alice_group
        .create_message(&alice.provider, &alice.signer, plaintext)
        .unwrap();
    let app_payload = mls_message_out_bytes(app_message);
    let app_envelope = envelope(
        ROOM_ID.to_string(),
        MLS_GROUP_ID.to_string(),
        alice.device_ref.clone(),
        1,
        LogEntryKind::Application,
        app_payload.clone(),
    );
    let appended = server
        .append_event(AppendEventRequest {
            room_id: ROOM_ID.to_string(),
            sender: alice.device_ref.clone(),
            envelope: app_envelope,
            idempotency_key: "real_mls_app_message".to_string(),
        })
        .unwrap();
    assert_eq!(appended.seq, 2);

    let bob_page = server.sync_events(ROOM_ID, &bob.device_ref, 1).unwrap();
    assert_eq!(bob_page.entries.len(), 1);
    assert_eq!(bob_page.entries[0].seq, appended.seq);
    assert_eq!(bob_page.entries[0].kind, LogEntryKind::Application);
    assert_eq!(bob_page.entries[0].envelope.payload, app_payload);

    let processed = bob_group
        .process_message(&bob.provider, protocol_message_from_bytes(&app_payload))
        .unwrap();
    let ProcessedMessageContent::ApplicationMessage(message) = processed.into_content() else {
        panic!("expected decrypted application message");
    };
    assert_eq!(message.into_bytes(), plaintext);
}

struct TestMlsDevice {
    provider: OpenMlsRustCrypto,
    account_secret: NostrSecretKey,
    device_ref: DeviceRef,
    credential_with_key: CredentialWithKey,
    signer: SignatureKeyPair,
}

impl TestMlsDevice {
    fn new(account_secret_bytes: [u8; NOSTR_SECRET_KEY_BYTES], device_id: &str) -> TestMlsDevice {
        let provider = OpenMlsRustCrypto::default();
        let signer = SignatureKeyPair::new(CIPHERSUITE.signature_algorithm()).unwrap();
        signer.store(provider.storage()).unwrap();
        let account_secret = NostrSecretKey::from_bytes(account_secret_bytes).unwrap();
        let account_public_key = account_secret.public_key();
        let credential = FiniteDeviceCredentialV1::sign(
            &account_secret,
            device_id,
            signer.to_public_vec(),
            NOW - 60,
            NOW + 60,
        )
        .unwrap();
        credential
            .verify_expected(ExpectedDeviceCredential {
                account_public_key,
                device_id,
                mls_leaf_signing_public_key: signer.public(),
                now_unix_seconds: NOW,
            })
            .unwrap();
        let credential_with_key = credential.to_openmls_credential_with_key();
        assert_eq!(
            credential_with_key.signature_key.as_slice(),
            signer.public()
        );

        TestMlsDevice {
            provider,
            account_secret,
            device_ref: device(hex_lower(account_public_key.as_bytes()), device_id),
            credential_with_key,
            signer,
        }
    }

    fn key_package_bundle(&self) -> KeyPackageBundle {
        KeyPackage::builder()
            .build(
                CIPHERSUITE,
                &self.provider,
                &self.signer,
                self.credential_with_key.clone(),
            )
            .unwrap()
    }
}

struct KeyPackageMetadata {
    key_package_ref: String,
    key_package_hash: String,
}

impl KeyPackageMetadata {
    fn from_bundle(device: &TestMlsDevice, key_package: &KeyPackageBundle) -> Self {
        let key_package_bytes = key_package
            .key_package()
            .tls_serialize_detached()
            .expect("KeyPackage should serialize");
        let key_package_ref = key_package
            .key_package()
            .hash_ref(device.provider.crypto())
            .expect("KeyPackage ref should hash");

        Self {
            key_package_ref: hex_lower(key_package_ref.as_slice()),
            key_package_hash: message_id_for_bytes(&key_package_bytes),
        }
    }
}

fn welcome_from_bytes(bytes: &[u8]) -> Welcome {
    let message = mls_message_in_from_bytes(bytes);
    let MlsMessageBodyIn::Welcome(welcome) = message.extract() else {
        panic!("expected a Welcome message");
    };
    welcome
}

fn ratchet_tree_from_bytes(bytes: &[u8]) -> RatchetTreeIn {
    assert!(!bytes.is_empty());
    RatchetTreeIn::tls_deserialize_exact(bytes).expect("serialized ratchet tree should parse")
}

fn protocol_message_from_bytes(bytes: &[u8]) -> ProtocolMessage {
    mls_message_in_from_bytes(bytes)
        .try_into_protocol_message()
        .expect("expected a protocol message")
}

fn mls_message_out_bytes(message: MlsMessageOut) -> Vec<u8> {
    let bytes = message.to_bytes().expect("MlsMessageOut should serialize");
    assert!(!bytes.is_empty());
    bytes
}

fn mls_message_in_from_bytes(mut bytes: &[u8]) -> MlsMessageIn {
    assert!(!bytes.is_empty());
    MlsMessageIn::tls_deserialize(&mut bytes)
        .expect("serialized MlsMessageOut should parse as MlsMessageIn")
}

fn assert_verified_member(group: &MlsGroup, device: &TestMlsDevice) {
    let mut verified_count = 0u32;
    for member in group.members() {
        let credential = FiniteDeviceCredentialV1::from_credential(member.credential).unwrap();
        if credential.device_id() == device.device_ref.device_id {
            credential
                .verify_expected(ExpectedDeviceCredential {
                    account_public_key: device.account_secret.public_key(),
                    device_id: &device.device_ref.device_id,
                    mls_leaf_signing_public_key: &member.signature_key,
                    now_unix_seconds: NOW,
                })
                .unwrap();
            verified_count += 1;
        }
    }
    assert_eq!(verified_count, 1);
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
