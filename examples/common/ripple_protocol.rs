// token, iterations, target, challenge = issue demand for proof of work
// token, response = give solution to proof of work
// token, result = report result of pow

//------------------------------------------------------------------------------

/// Provides the current ephemeral key for a validator.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmManifest {
    /// A Manifest object in the Ripple serialization format.
    #[prost(bytes = "vec", required, tag = "1")]
    pub stobject: ::prost::alloc::vec::Vec<u8>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmManifests {
    #[prost(message, repeated, tag = "1")]
    pub list: ::prost::alloc::vec::Vec<TmManifest>,
    /// The manifests sent when a peer first connects to another peer are `history`.
    #[deprecated]
    #[prost(bool, optional, tag = "2")]
    pub history: ::core::option::Option<bool>,
}
//------------------------------------------------------------------------------

/// The status of a node in our cluster
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmClusterNode {
    #[prost(string, required, tag = "1")]
    pub public_key: ::prost::alloc::string::String,
    #[prost(uint32, required, tag = "2")]
    pub report_time: u32,
    #[prost(uint32, required, tag = "3")]
    pub node_load: u32,
    #[prost(string, optional, tag = "4")]
    pub node_name: ::core::option::Option<::prost::alloc::string::String>,
    #[prost(string, optional, tag = "5")]
    pub address: ::core::option::Option<::prost::alloc::string::String>,
}
/// Sources that are placing load on the server
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmLoadSource {
    #[prost(string, required, tag = "1")]
    pub name: ::prost::alloc::string::String,
    #[prost(uint32, required, tag = "2")]
    pub cost: u32,
    /// number of connections
    #[prost(uint32, optional, tag = "3")]
    pub count: ::core::option::Option<u32>,
}
/// The status of all nodes in the cluster
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmCluster {
    #[prost(message, repeated, tag = "1")]
    pub cluster_nodes: ::prost::alloc::vec::Vec<TmClusterNode>,
    #[prost(message, repeated, tag = "2")]
    pub load_sources: ::prost::alloc::vec::Vec<TmLoadSource>,
}
/// Node public key
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmLink {
    /// node public key
    #[deprecated]
    #[prost(bytes = "vec", required, tag = "1")]
    pub node_pub_key: ::prost::alloc::vec::Vec<u8>,
}
/// Request info on shards held
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmGetPeerShardInfo {
    /// number of hops to travel
    #[deprecated]
    #[prost(uint32, required, tag = "1")]
    pub hops: u32,
    /// true if last link in the peer chain
    #[deprecated]
    #[prost(bool, optional, tag = "2")]
    pub last_link: ::core::option::Option<bool>,
    /// public keys used to route messages
    #[deprecated]
    #[prost(message, repeated, tag = "3")]
    pub peer_chain: ::prost::alloc::vec::Vec<TmLink>,
}
/// Info about shards held
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmPeerShardInfo {
    /// rangeSet of shard indexes
    #[deprecated]
    #[prost(string, required, tag = "1")]
    pub shard_indexes: ::prost::alloc::string::String,
    /// node public key
    #[deprecated]
    #[prost(bytes = "vec", optional, tag = "2")]
    pub node_pub_key: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// ipv6 or ipv4 address
    #[deprecated]
    #[prost(string, optional, tag = "3")]
    pub endpoint: ::core::option::Option<::prost::alloc::string::String>,
    /// true if last link in the peer chain
    #[deprecated]
    #[prost(bool, optional, tag = "4")]
    pub last_link: ::core::option::Option<bool>,
    /// public keys used to route messages
    #[deprecated]
    #[prost(message, repeated, tag = "5")]
    pub peer_chain: ::prost::alloc::vec::Vec<TmLink>,
}
/// Peer public key
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmPublicKey {
    #[prost(bytes = "vec", required, tag = "1")]
    pub public_key: ::prost::alloc::vec::Vec<u8>,
}
/// Request peer shard information
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmGetPeerShardInfoV2 {
    /// Peer public keys used to route messages
    #[prost(message, repeated, tag = "1")]
    pub peer_chain: ::prost::alloc::vec::Vec<TmPublicKey>,
    /// Remaining times to relay
    #[prost(uint32, required, tag = "2")]
    pub relays: u32,
}
/// Peer shard information
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmPeerShardInfoV2 {
    /// Message creation time
    #[prost(uint32, required, tag = "1")]
    pub timestamp: u32,
    /// Incomplete shards being acquired or verified
    #[prost(message, repeated, tag = "2")]
    pub incomplete: ::prost::alloc::vec::Vec<tm_peer_shard_info_v2::TmIncomplete>,
    /// Verified immutable shards (RangeSet)
    #[prost(string, optional, tag = "3")]
    pub finalized: ::core::option::Option<::prost::alloc::string::String>,
    /// Public key of node that authored the shard info
    #[prost(bytes = "vec", required, tag = "4")]
    pub public_key: ::prost::alloc::vec::Vec<u8>,
    /// Digital signature of node that authored the shard info
    #[prost(bytes = "vec", required, tag = "5")]
    pub signature: ::prost::alloc::vec::Vec<u8>,
    /// Peer public keys used to route messages
    #[prost(message, repeated, tag = "6")]
    pub peer_chain: ::prost::alloc::vec::Vec<TmPublicKey>,
}
/// Nested message and enum types in `TMPeerShardInfoV2`.
pub mod tm_peer_shard_info_v2 {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TmIncomplete {
        #[prost(uint32, required, tag = "1")]
        pub shard_index: u32,
        #[prost(uint32, required, tag = "2")]
        pub state: u32,
        /// State completion percent, 1 - 100
        #[prost(uint32, optional, tag = "3")]
        pub progress: ::core::option::Option<u32>,
    }
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmTransaction {
    #[prost(bytes = "vec", required, tag = "1")]
    pub raw_transaction: ::prost::alloc::vec::Vec<u8>,
    #[prost(enumeration = "TransactionStatus", required, tag = "2")]
    pub status: i32,
    #[prost(uint64, optional, tag = "3")]
    pub receive_timestamp: ::core::option::Option<u64>,
    /// not applied to open ledger
    #[prost(bool, optional, tag = "4")]
    pub deferred: ::core::option::Option<bool>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmTransactions {
    #[prost(message, repeated, tag = "1")]
    pub transactions: ::prost::alloc::vec::Vec<TmTransaction>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmStatusChange {
    #[prost(enumeration = "NodeStatus", optional, tag = "1")]
    pub new_status: ::core::option::Option<i32>,
    #[prost(enumeration = "NodeEvent", optional, tag = "2")]
    pub new_event: ::core::option::Option<i32>,
    #[prost(uint32, optional, tag = "3")]
    pub ledger_seq: ::core::option::Option<u32>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub ledger_hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub ledger_hash_previous: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(uint64, optional, tag = "6")]
    pub network_time: ::core::option::Option<u64>,
    #[prost(uint32, optional, tag = "7")]
    pub first_seq: ::core::option::Option<u32>,
    #[prost(uint32, optional, tag = "8")]
    pub last_seq: ::core::option::Option<u32>,
}
/// Announce to the network our position on a closing ledger
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmProposeSet {
    #[prost(uint32, required, tag = "1")]
    pub propose_seq: u32,
    /// the hash of the ledger we are proposing
    #[prost(bytes = "vec", required, tag = "2")]
    pub current_tx_hash: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", required, tag = "3")]
    pub node_pub_key: ::prost::alloc::vec::Vec<u8>,
    #[prost(uint32, required, tag = "4")]
    pub close_time: u32,
    /// signature of above fields
    #[prost(bytes = "vec", required, tag = "5")]
    pub signature: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", required, tag = "6")]
    pub previousledger: ::prost::alloc::vec::Vec<u8>,
    /// not required if number is large
    #[prost(bytes = "vec", repeated, tag = "10")]
    pub added_transactions: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    /// not required if number is large
    #[prost(bytes = "vec", repeated, tag = "11")]
    pub removed_transactions: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    /// node vouches signature is correct
    #[deprecated]
    #[prost(bool, optional, tag = "7")]
    pub checked_signature: ::core::option::Option<bool>,
    /// Number of hops traveled
    #[deprecated]
    #[prost(uint32, optional, tag = "12")]
    pub hops: ::core::option::Option<u32>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmHaveTransactionSet {
    #[prost(enumeration = "TxSetStatus", required, tag = "1")]
    pub status: i32,
    #[prost(bytes = "vec", required, tag = "2")]
    pub hash: ::prost::alloc::vec::Vec<u8>,
}
/// Validator list (UNL)
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmValidatorList {
    #[prost(bytes = "vec", required, tag = "1")]
    pub manifest: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", required, tag = "2")]
    pub blob: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", required, tag = "3")]
    pub signature: ::prost::alloc::vec::Vec<u8>,
    #[prost(uint32, required, tag = "4")]
    pub version: u32,
}
/// Validator List v2
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ValidatorBlobInfo {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub manifest: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", required, tag = "2")]
    pub blob: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", required, tag = "3")]
    pub signature: ::prost::alloc::vec::Vec<u8>,
}
/// Collection of Validator List v2 (UNL)
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmValidatorListCollection {
    #[prost(uint32, required, tag = "1")]
    pub version: u32,
    #[prost(bytes = "vec", required, tag = "2")]
    pub manifest: ::prost::alloc::vec::Vec<u8>,
    #[prost(message, repeated, tag = "3")]
    pub blobs: ::prost::alloc::vec::Vec<ValidatorBlobInfo>,
}
/// Used to sign a final closed ledger after reprocessing
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmValidation {
    /// The serialized validation
    #[prost(bytes = "vec", required, tag = "1")]
    pub validation: ::prost::alloc::vec::Vec<u8>,
    /// node vouches signature is correct
    #[deprecated]
    #[prost(bool, optional, tag = "2")]
    pub checked_signature: ::core::option::Option<bool>,
    /// Number of hops traveled
    #[deprecated]
    #[prost(uint32, optional, tag = "3")]
    pub hops: ::core::option::Option<u32>,
}
/// An array of Endpoint messages
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmEndpoints {
    /// This field is used to allow the TMEndpoints message format to be
    /// modified as necessary in the future.
    #[prost(uint32, required, tag = "1")]
    pub version: u32,
    #[prost(message, repeated, tag = "3")]
    pub endpoints_v2: ::prost::alloc::vec::Vec<tm_endpoints::TmEndpointv2>,
}
/// Nested message and enum types in `TMEndpoints`.
pub mod tm_endpoints {
    /// An update to the Endpoint type that uses a string
    /// to represent endpoints, thus allowing ipv6 or ipv4 addresses
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TmEndpointv2 {
        #[prost(string, required, tag = "1")]
        pub endpoint: ::prost::alloc::string::String,
        #[prost(uint32, required, tag = "2")]
        pub hops: u32,
    }
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmIndexedObject {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub node_id: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub index: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub data: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(uint32, optional, tag = "5")]
    pub ledger_seq: ::core::option::Option<u32>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmGetObjectByHash {
    #[prost(enumeration = "tm_get_object_by_hash::ObjectType", required, tag = "1")]
    pub r#type: i32,
    /// is this a query or a reply?
    #[prost(bool, required, tag = "2")]
    pub query: bool,
    /// used to match replies to queries
    #[prost(uint32, optional, tag = "3")]
    pub seq: ::core::option::Option<u32>,
    /// the hash of the ledger these queries are for
    #[prost(bytes = "vec", optional, tag = "4")]
    pub ledger_hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// return related nodes
    #[prost(bool, optional, tag = "5")]
    pub fat: ::core::option::Option<bool>,
    /// the specific objects requested
    #[prost(message, repeated, tag = "6")]
    pub objects: ::prost::alloc::vec::Vec<TmIndexedObject>,
}
/// Nested message and enum types in `TMGetObjectByHash`.
pub mod tm_get_object_by_hash {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum ObjectType {
        OtUnknown = 0,
        OtLedger = 1,
        OtTransaction = 2,
        OtTransactionNode = 3,
        OtStateNode = 4,
        OtCasObject = 5,
        OtFetchPack = 6,
        OtTransactions = 7,
    }
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmLedgerNode {
    #[prost(bytes = "vec", required, tag = "1")]
    pub nodedata: ::prost::alloc::vec::Vec<u8>,
    /// missing for ledger base data
    #[prost(bytes = "vec", optional, tag = "2")]
    pub nodeid: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmGetLedger {
    #[prost(enumeration = "TmLedgerInfoType", required, tag = "1")]
    pub itype: i32,
    #[prost(enumeration = "TmLedgerType", optional, tag = "2")]
    pub ltype: ::core::option::Option<i32>,
    /// Can also be the transaction set hash if liTS_CANDIDATE
    #[prost(bytes = "vec", optional, tag = "3")]
    pub ledger_hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(uint32, optional, tag = "4")]
    pub ledger_seq: ::core::option::Option<u32>,
    #[prost(bytes = "vec", repeated, tag = "5")]
    pub node_i_ds: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    #[prost(uint64, optional, tag = "6")]
    pub request_cookie: ::core::option::Option<u64>,
    #[prost(enumeration = "TmQueryType", optional, tag = "7")]
    pub query_type: ::core::option::Option<i32>,
    /// How deep to go, number of extra levels
    #[prost(uint32, optional, tag = "8")]
    pub query_depth: ::core::option::Option<u32>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmLedgerData {
    #[prost(bytes = "vec", required, tag = "1")]
    pub ledger_hash: ::prost::alloc::vec::Vec<u8>,
    #[prost(uint32, required, tag = "2")]
    pub ledger_seq: u32,
    #[prost(enumeration = "TmLedgerInfoType", required, tag = "3")]
    pub r#type: i32,
    #[prost(message, repeated, tag = "4")]
    pub nodes: ::prost::alloc::vec::Vec<TmLedgerNode>,
    #[prost(uint32, optional, tag = "5")]
    pub request_cookie: ::core::option::Option<u32>,
    #[prost(enumeration = "TmReplyError", optional, tag = "6")]
    pub error: ::core::option::Option<i32>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmPing {
    #[prost(enumeration = "tm_ping::PingType", required, tag = "1")]
    pub r#type: i32,
    /// detect stale replies, ensure other side is reading
    #[prost(uint32, optional, tag = "2")]
    pub seq: ::core::option::Option<u32>,
    /// know when we think we sent the ping
    #[prost(uint64, optional, tag = "3")]
    pub ping_time: ::core::option::Option<u64>,
    #[prost(uint64, optional, tag = "4")]
    pub net_time: ::core::option::Option<u64>,
}
/// Nested message and enum types in `TMPing`.
pub mod tm_ping {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum PingType {
        /// we want a reply
        PtPing = 0,
        /// this is a reply
        PtPong = 1,
    }
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmSquelch {
    /// squelch if true, otherwise unsquelch
    #[prost(bool, required, tag = "1")]
    pub squelch: bool,
    /// validator's public key
    #[prost(bytes = "vec", required, tag = "2")]
    pub validator_pub_key: ::prost::alloc::vec::Vec<u8>,
    /// squelch duration in seconds
    #[prost(uint32, optional, tag = "3")]
    pub squelch_duration: ::core::option::Option<u32>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmProofPathRequest {
    #[prost(bytes = "vec", required, tag = "1")]
    pub key: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", required, tag = "2")]
    pub ledger_hash: ::prost::alloc::vec::Vec<u8>,
    #[prost(enumeration = "TmLedgerMapType", required, tag = "3")]
    pub r#type: i32,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmProofPathResponse {
    #[prost(bytes = "vec", required, tag = "1")]
    pub key: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", required, tag = "2")]
    pub ledger_hash: ::prost::alloc::vec::Vec<u8>,
    #[prost(enumeration = "TmLedgerMapType", required, tag = "3")]
    pub r#type: i32,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub ledger_header: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", repeated, tag = "5")]
    pub path: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    #[prost(enumeration = "TmReplyError", optional, tag = "6")]
    pub error: ::core::option::Option<i32>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmReplayDeltaRequest {
    #[prost(bytes = "vec", required, tag = "1")]
    pub ledger_hash: ::prost::alloc::vec::Vec<u8>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmReplayDeltaResponse {
    #[prost(bytes = "vec", required, tag = "1")]
    pub ledger_hash: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub ledger_header: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", repeated, tag = "3")]
    pub transaction: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    #[prost(enumeration = "TmReplyError", optional, tag = "4")]
    pub error: ::core::option::Option<i32>,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TmHaveTransactions {
    #[prost(bytes = "vec", repeated, tag = "1")]
    pub hashes: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
}
/// Unused numbers in the list below may have been used previously. Please don't
/// reassign them for reuse unless you are 100% certain that there won't be a
/// conflict. Even if you're sure, it's probably best to assign a new type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum MessageType {
    MtManifests = 2,
    MtPing = 3,
    MtCluster = 5,
    MtEndpoints = 15,
    MtTransaction = 30,
    MtGetLedger = 31,
    MtLedgerData = 32,
    MtProposeLedger = 33,
    MtStatusChange = 34,
    MtHaveSet = 35,
    MtValidation = 41,
    MtGetObjects = 42,
    MtGetShardInfo = 50,
    MtShardInfo = 51,
    MtGetPeerShardInfo = 52,
    MtPeerShardInfo = 53,
    MtValidatorlist = 54,
    MtSquelch = 55,
    MtValidatorlistcollection = 56,
    MtProofPathReq = 57,
    MtProofPathResponse = 58,
    MtReplayDeltaReq = 59,
    MtReplayDeltaResponse = 60,
    MtGetPeerShardInfoV2 = 61,
    MtPeerShardInfoV2 = 62,
    MtHaveTransactions = 63,
    MtTransactions = 64,
}
// A transaction can have only one input and one output.
// If you want to send an amount that is greater than any single address of yours
// you must first combine coins from one address to another.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum TransactionStatus {
    /// origin node did/could not validate
    TsNew = 1,
    /// scheduled to go in this ledger
    TsCurrent = 2,
    /// in a closed ledger
    TsCommited = 3,
    TsRejectConflict = 4,
    TsRejectInvalid = 5,
    TsRejectFunds = 6,
    TsHeldSeq = 7,
    /// held for future ledger
    TsHeldLedger = 8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum NodeStatus {
    /// acquiring connections
    NsConnecting = 1,
    /// convinced we are connected to the real network
    NsConnected = 2,
    /// we know what the previous ledger is
    NsMonitoring = 3,
    /// we have the full ledger contents
    NsValidating = 4,
    /// node is shutting down
    NsShutting = 5,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum NodeEvent {
    /// closing a ledger because its close time has come
    NeClosingLedger = 1,
    /// accepting a closed ledger, we have finished computing it
    NeAcceptedLedger = 2,
    /// changing due to network consensus
    NeSwitchedLedger = 3,
    NeLostSync = 4,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum TxSetStatus {
    /// We have this set locally
    TsHave = 1,
    /// We have a peer with this set
    TsCanGet = 2,
    /// We need this set and can't get it
    TsNeed = 3,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum TmLedgerInfoType {
    /// basic ledger info
    LiBase = 0,
    /// transaction node
    LiTxNode = 1,
    /// account state node
    LiAsNode = 2,
    /// candidate transaction set
    LiTsCandidate = 3,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum TmLedgerType {
    LtAccepted = 0,
    /// no longer supported
    LtCurrent = 1,
    LtClosed = 2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum TmQueryType {
    QtIndirect = 0,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum TmReplyError {
    /// We don't have the ledger you are asking about
    ReNoLedger = 1,
    /// We don't have any of the nodes you are asking for
    ReNoNode = 2,
    /// The request is wrong, e.g. wrong format
    ReBadRequest = 3,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum TmLedgerMapType {
    /// transaction map
    LmTranasction = 1,
    /// account state map
    LmAccountState = 2,
}
