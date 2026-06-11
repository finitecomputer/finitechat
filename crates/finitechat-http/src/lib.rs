use cgka_traits::transport::TransportMessage;
use cgka_traits::{GroupId, MemberId, MessageId};
use finitechat_proto::{
    RoomProtocol,ApplicationDeliveryPolicy, DeviceRef, MembershipDeltaV1, RoomLogEntry};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use transport_http_server::{
    HttpClaimedKeyPackage, HttpKeyPackageId, HttpPublishTarget, HttpSequence,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishMessageRequest {
    pub target: HttpPublishTarget,
    pub message: TransportMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiniteAccountRoomCommitProjection {
    pub entry: RoomLogEntry,
    pub membership_delta: MembershipDeltaV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationEffectRequest {
    pub message_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpApplicationDeliveryEffect {
    pub room_id: String,
    pub seq: HttpSequence,
    pub message_id: String,
    pub sender: DeviceRef,
    pub delivery_policy: ApplicationDeliveryPolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationEffectCountsResponse {
    pub push_outbox: u32,
    pub unread: u32,
    pub command_inbox: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupSyncRequest {
    pub group_id: GroupId,
    pub after_seq: HttpSequence,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<MemberId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxSyncRequest {
    pub recipient: MemberId,
    pub after_seq: HttpSequence,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeDeviceRequest {
    pub device: DeviceRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeDeviceResponse {
    pub revoked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObserveDeviceLivenessRequest {
    pub device: DeviceRef,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLivenessRecord {
    pub device: DeviceRef,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetDeviceLivenessRequest {
    pub device: DeviceRef,
    pub now_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetDeviceLivenessResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<DeviceLivenessRecord>,
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimKeyPackageRequest {
    pub owner: MemberId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpireKeyPackageLeaseRequest {
    pub key_package_id: HttpKeyPackageId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpireKeyPackageLeaseResponse {
    pub expired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimKeyPackagesRequest {
    pub owners: Vec<MemberId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPackageInventoryRequest {
    pub owner: MemberId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpKeyPackageInventory {
    pub owner: MemberId,
    pub available: u32,
    pub claimed: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpKeyPackageClaim {
    pub owner: MemberId,
    pub claimed: Option<HttpClaimedKeyPackage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveFanoutRoomRequest {
    pub fanout_id: String,
    pub target_owner: MemberId,
    pub room: HttpFanoutRoomPlan,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetFanoutRequest {
    pub fanout_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkFanoutPreparedRequest {
    pub fanout_id: String,
    pub room_id: GroupId,
    pub prepared_message_id: MessageId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkFanoutDoneRequest {
    pub fanout_id: String,
    pub room_id: GroupId,
    pub prepared_message_id: MessageId,
    pub accepted_seq: HttpSequence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpFanoutPlan {
    pub fanout_id: String,
    pub target_owner: MemberId,
    pub rooms: Vec<HttpFanoutRoomState>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpFanoutRoomState {
    pub plan: HttpFanoutRoomPlan,
    pub status: HttpFanoutRoomStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpFanoutRoomPlan {
    pub room_id: GroupId,
    pub key_package_id: HttpKeyPackageId,
    pub welcome_id: MessageId,
    pub commit_idempotency_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_key_package_id: Option<HttpKeyPackageId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpFanoutRoomStatus {
    Pending,
    Prepared {
        prepared_message_id: MessageId,
    },
    Done {
        prepared_message_id: MessageId,
        accepted_seq: HttpSequence,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateLinkSessionRequest {
    pub link_session_id: String,
    pub pairing_public_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetLinkSessionRequest {
    pub link_session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadLinkPayloadRequest {
    pub link_session_id: String,
    pub encrypted_payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimLinkPayloadRequest {
    pub link_session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimLinkPayloadResponse {
    pub encrypted_payload: Vec<u8>,
    pub claim_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckLinkPayloadRequest {
    pub link_session_id: String,
    pub claim_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckLinkPayloadResponse {
    pub acked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseLinkClaimRequest {
    pub link_session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseLinkClaimResponse {
    pub released: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpireLinkSessionRequest {
    pub link_session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpireLinkSessionResponse {
    pub expired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpLinkSessionRecord {
    pub link_session_id: String,
    pub pairing_public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_payload: Option<Vec<u8>>,
    pub state: HttpLinkSessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_token: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpLinkSessionState {
    Created,
    PayloadUploaded,
    Claimed,
    Delivered,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveAccountRoomRequest {
    pub account_id: String,
    pub room_id: String,
    pub record: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveAccountRoomResponse {
    pub saved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapAccountRoomRequest {
    pub room_id: String,
    pub mls_group_id: String,
    pub creator: DeviceRef,
    #[serde(default)]
    pub protocol: RoomProtocol,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapAccountRoomResponse {
    pub bootstrapped: bool,
}


#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListAccountRoomDirectoryRequest {
    pub account_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_room_id: Option<String>,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListAccountRoomDirectoryResponse {
    pub rooms: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after_room_id: Option<String>,
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimWelcomesRequest {
    pub recipient: MemberId,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpClaimedWelcome {
    pub seq: HttpSequence,
    pub message: TransportMessage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckWelcomeRequest {
    pub message_id: MessageId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckWelcomeResponse {
    pub acked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaveRoomRequest {
    pub room_id: String,
    pub sender: DeviceRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaveRoomResponse {
    pub left: bool,
    pub departed_at_seq: HttpSequence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRoomAdminsRequest {
    pub room_id: String,
    pub sender: DeviceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoke: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRoomAdminsResponse {
    pub admins: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportInvalidCommitRequest {
    pub room_id: String,
    pub reporter: DeviceRef,
    pub offending_seq: HttpSequence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportInvalidCommitResponse {
    pub reported: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishKeyPackageResponse {
    pub published: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub kind: String,
    pub error: String,
}
