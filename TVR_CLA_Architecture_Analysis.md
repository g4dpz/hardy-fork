# Hardy DTN - TVR and CLA Architecture Analysis

*Analysis of Hardy's Time-Variant Routing (TVR) and Convergence Layer Adapter (CLA) architecture for LTP-CLA implementation*

## Executive Summary

Hardy implements a modular Bundle Processing Agent (BPA) architecture with clean separation between routing decisions (TVR), convergence layer transport (CLA), and bundle processing. This analysis documents the key architectural patterns, interfaces, and extension points relevant for implementing a Licklider Transmission Protocol (LTP) Convergence Layer Adapter.

Throughout this document, code blocks that reproduce Hardy's real traits and types are verified against the current source. Illustrative code for the not-yet-existing LTP-CLA is clearly labelled **proposed / illustrative** and is written to be consistent with the real APIs it builds on.

## 1. TVR (Time-Variant Routing) Implementation

### Architecture Overview

TVR is implemented as a standalone binary (`tvr/` crate) that operates independently from CLAs. It does not run in-process with the BPA: it connects over gRPC and registers as a `RoutingAgent` using `hardy_proto::client::RemoteBpa`. Concretely, `tvr/src/main.rs` constructs `RemoteBpa::new(bpa_address)` and calls `register_routing_agent(name, agent)`, which returns the BPA's node IDs. The BPA then drives `RoutingAgent::on_register(sink, node_ids)` on the agent, and the agent stores the `Arc<dyn RoutingSink>` for its lifetime.

### Core Components

**TvrAgent** (`tvr/src/contacts.rs`)
- Implements the `RoutingAgent` trait (`on_register`, `on_unregister`).
- On `on_register`, stores the injected `Box<dyn RoutingSink>` (converted to `Arc<dyn RoutingSink>`) in a `spin::Once`; it does not own or manage a RIB directly — the RIB lives in the BPA, and the agent only pushes routes to it through the Sink.
- Holds a `default_priority: u32` applied to contacts without an explicit priority, and a `scheduler: SchedulerHandle` for submitting contact operations.

**Scheduler** (`tvr/src/scheduler.rs`)
- Event-driven timeline manager started after registration (it needs the Sink).
- Activates and deactivates contacts based on their time windows, pushing `add_route` / `remove_route` calls through the Sink as contacts become active or inactive.

**Contact Management** (`tvr/src/contacts.rs`)
- A `Contact` pairs an `EidPattern` with a `RouteAction` (`Via`/`Reflect`/`Drop`), an optional priority, an optional bandwidth and delay, and a `Schedule`.
- Supports three schedule types via the `Schedule` enum:
  - `Permanent` (always active, like a static route)
  - `OneShot { start, end }` (a single time window)
  - `Recurring { cron, duration, until }` (cron-style recurrence)
- Runtime contact updates arrive over a gRPC session server (`tvr/src/server.rs`), which feeds the same scheduler path as the file-based contact plan.

### Standards Compliance

- Implements scheduling based on contact plans.
- Supports RFC 9171 routing concepts with time-variant extensions.
- Compatible with standard Bundle Protocol v7 routing semantics.

### Key Interface: RoutingSink

The real `RoutingSink` trait (`bpa/src/routing/agent/mod.rs`) is route-centric, not `Route`-object-centric. There is no `install_route` / `withdraw_route` and no `Route` type:

```rust
#[async_trait]
pub trait RoutingSink: Send + Sync {
    /// Explicitly unregisters the associated routing agent from the BPA.
    async fn unregister(&self);

    /// Adds a route to the RIB.
    /// Returns `true` if the route was newly inserted, `false` if it already existed.
    async fn add_route(
        &self,
        pattern: EidPattern,
        action: RouteAction,
        priority: u32,
    ) -> Result<bool>;

    /// Removes a specific route from the RIB.
    /// Only removes routes matching all three fields and this agent's source name.
    /// Returns `true` if the route was found and removed.
    async fn remove_route(
        &self,
        pattern: &EidPattern,
        action: &RouteAction,
        priority: u32,
    ) -> Result<bool>;
}
```

`RouteAction` (`bpa/src/routing/action.rs`, re-exported as `hardy_bpa::routing::RouteAction`) is `enum { Via(Eid), Reflect, Drop(Option<ReasonCode>) }`.

**Integration Pattern**: TVR adds and removes routes identified by the triple `(EidPattern, RouteAction, priority)` as contacts become active or inactive; it never installs a `Route` object or withdraws by bare `Eid`. The Sink injects the agent's registered name as the route *source*, so an agent can only add or remove routes attributed to itself, and dropping the Sink removes all of that agent's routes automatically. Routing agents are purely push-based: the BPA never calls back into the agent to request work (unlike CLAs, which expose a `forward` method).

## 2. CLA Architecture

### Core Trait Interface

All CLAs implement the `Cla` trait defined in `bpa/src/cla/mod.rs`:

```rust
#[async_trait]
pub trait Cla: Send + Sync {
    /// Registration lifecycle. The sink MUST be stored for the CLA's lifetime.
    async fn on_register(
        &self,
        sink: Box<dyn Sink>,
        node_ids: &[NodeId],
        max_bundle_size: Option<NonZeroU64>,
    );

    /// Cleanup on disconnection.
    async fn on_unregister(&self);

    /// Forward a bundle, delivered as a stream of segments, to a CLA
    /// address over a given lane.
    async fn forward(
        &self,
        lane: Option<u32>,
        cla_addr: &ClaAddress,
        bundle_id: &Id,
        total_len: u64,
        stream: &mut dyn crate::stream::Receiver<Segment>,
    ) -> Result<ForwardBundleResult>;
}
```

The `max_bundle_size` passed to `on_register` is the *effective* cap: the minimum of the BPA's configured cap and the limit this CLA declared at registration (`None` means no cap was negotiated). Dispatch enforces the BPA's configured cap, not the folded per-CLA value.

### Lane model

`ClaInit::lane_count` and `forward`'s `lane: Option<u32>` parameter together describe Hardy's lane model (`bpa/src/cla/mod.rs`, `bpa/src/cla/egress_queue.rs`). Lanes are parallel transport channels — QUIC streams, DSCP classes, separate TCP connections — that carry in-flight transfers with no implicit priority among them.

- `lane_count = None` declares no limit: the CLA is effectively a datagram CL drawing from an infinite pool, and every `forward` arrives with `lane = None`, each transfer travelling on its own fresh lane.
- `lane_count = Some(n)` declares `n` explicit lanes indexed `0..n` (a zero count is unrepresentable). `forward` with `Some(i)` directs the transfer onto lane `i`.

Scheduling and prioritisation live in the BPA's egress policy, which decides what is forwarded onto each lane; the CLA simply transmits what arrives on a lane, and one lane's in-flight transfer must not head-of-line block another's. As a safety net, a CLA that declares explicit lanes but still receives `None` should pick a lane arbitrarily rather than fail the transfer. A queue is instantiated eagerly per declared lane, so the declared count is clamped to 256 to keep an absurd declaration from becoming a resource bomb (`egress_queue.rs`, `MAX_EAGER_LANE_QUEUES`).

### Sink Interface: CLA → BPA Communication

CLAs communicate back to the BPA via the `Sink` trait (`bpa/src/cla/mod.rs`):

```rust
#[async_trait]
pub trait Sink: Send + Sync {
    /// Explicit disconnection (equivalent to dropping the Sink).
    async fn unregister(&self);

    /// Dispatch a received bundle (as a stream of segments) to the BPA's Dispatcher.
    /// Returns the acceptance verdict for the transfer.
    async fn dispatch(
        &self,
        peer_node: Option<&NodeId>,
        peer_addr: Option<&ClaAddress>,
        stream: &mut dyn Receiver<Segment>,
    ) -> Result<Acceptance>;

    /// Manage peer discovery/topology.
    async fn add_peer(&self, cla_addr: ClaAddress, node_ids: &[NodeId]) -> Result<bool>;
    async fn remove_peer(&self, cla_addr: &ClaAddress) -> Result<bool>;

    /// Report the final outcome of a transfer previously answered Accepted.
    async fn transfer_outcome(&self, bundle_id: &Id, outcome: TransferOutcome) -> Result<()>;
}
```

### Acceptance: the dispatch verdict

`dispatch` returns `Result<Acceptance>`, not `Result<()>`. `Acceptance` (`bpa/src/cla/mod.rs`) is the transfer-acknowledgement verdict and `dispatch`'s only carrier:

```rust
#[must_use]
pub enum Acceptance {
    Accepted,
    Refused,
}
```

- `Accepted` means the CLA may acknowledge the transfer to its peer. Acceptance *includes* internal disposal: a bundle that is invalid, unwanted, or a duplicate is accepted and then dropped inside the BPA (with status reports where requested). Refusing such a bundle would only invite the previous node's custody/retransmission machinery to resend byte-identical content that can never succeed, so the first BPA to detect the problem accepts and terminates it.
- `Refused` means the CLA must not acknowledge the transfer, leaving its peer responsible for the bundle. Refusal is reserved for transfers the BPA could not take at all — a truncated stream (a resend may succeed) or a bundle over the BPA's size cap (the peer must stop offering it).

Refusal is a normal protocol outcome, not an error. `Err(_)` from `dispatch` is reserved for faults that make the sink unusable (the registration or the BPA has gone away) and is never a verdict. A producer that drops its sender before `Segment::Final` has truncated the bundle, and the implementation surfaces that as `Refused`, never a silent acceptance.

### Registration & Lifecycle Management

1. **Registration**: the public entry point is `BpaRegistration::register_cla(name, cla, policy, init)` (`bpa/src/bpa.rs`), which drives `Cla::on_register` to completion before returning and yields the BPA's node IDs. (`ClaRegistry::register` is an internal delegate, not the public API.)

   ```rust
   async fn register_cla(
       &self,
       name: String,
       cla: Arc<dyn Cla>,
       policy: Option<Arc<dyn FlowControllerFactory>>,
       init: ClaInit,
   ) -> cla::Result<Vec<NodeId>>;
   ```

2. **Initialization**: the BPA calls `on_register(sink, node_ids, max_bundle_size)`, where `max_bundle_size` is the effective folded cap described above.
3. **Active Operation**: the CLA must store the `Sink` for its entire lifetime.
4. **Disconnection**: dropping the `Sink` (or calling `sink.unregister()`) triggers automatic unregistration.
5. **Cleanup**: the BPA calls `on_unregister()` for resource cleanup; afterwards the Sink is non-functional.

`register_cla` is available both in-process (any `Bpa` implements `BpaRegistration`) and out-of-process over gRPC: `hardy_proto::client::RemoteBpa` also implements `BpaRegistration::register_cla`, so a gRPC-attached CLA registers the same way TVR registers its routing agent (policy is not carried over the gRPC path).

### Address Type System

CLAs declare their address type and parallelism during registration via `ClaInit`:

```rust
pub struct ClaInit {
    pub address_type: Option<ClaAddressType>,
    pub lane_count: Option<NonZeroU32>,
    pub max_bundle_size: Option<NonZeroU64>,
}
```

The real address enums (`bpa/src/cla/mod.rs`) have exactly two variants each — there is **no `Ltp` variant** in the current code, and `Private` carries raw address bytes, not a type-id payload:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaAddressType {
    Tcp,
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaAddress {
    Tcp(core::net::SocketAddr),
    Private(Bytes),
}
```

#### Proposed (not yet in Hardy): LTP addressing

An LTP-CLA needs a way to name an LTP engine as a peer address. Two designs are available:

1. **No core change** — carry the LTP engine identifier inside the existing `Private(Bytes)` variant. The CLA owns the byte encoding, declares `address_type: Some(ClaAddressType::Private)`, and nothing in the core enums changes. This is the lowest-friction option and is recommended unless core integration is specifically desired.
2. **New core variants** (proposed) — add an `Ltp` variant to both enums:

   ```rust
   // PROPOSED — does not exist in Hardy today.
   pub enum ClaAddressType {
       Tcp,
       Ltp,     // proposed
       Private,
   }

   pub enum ClaAddress {
       Tcp(core::net::SocketAddr),
       Ltp(LtpEngineId),  // proposed
       Private(Bytes),
   }
   ```

   This is cleaner at the type level but touches shared core enums (and their serde, conversions, and `Display`), so it is a larger change.

## 3. Existing CLA Implementation Patterns

### TCPCLv4: Complex Protocol Implementation

**Location**: `tcpclv4/` crate
**Pattern**: Full RFC 9174 implementation with:
- TLS support and authentication
- Connection pooling and session management
- Keepalive mechanisms and timeout handling
- Bidirectional segment streaming
- Flow control and transfer acknowledgments

**Key Architectural Elements**:
- A connection registry managing connection pools per peer
- Session state machines for protocol negotiation
- Deferred transfer outcomes reported via `Sink::transfer_outcome`
- Connection rate limiting and resource management

**Illustrative patterns** (the following snippets are illustrative pseudo-code showing the *shape* of the pattern, not verified field names from `tcpclv4/`; names like `transfer_mru` / `segment_mru` are illustrative):

```rust
// Illustrative: store the sink during registration.
self.inner.call_once(|| Inner {
    sink: sink.into(),
    node_ids: node_ids.into(),
    transfer_mru,   // illustrative
    segment_mru,    // illustrative
});

// Illustrative: deferred transfer handling, consistent with the real
// ForwardBundleResult::Accepted + Sink::transfer_outcome contract.
self.tasks.spawn(async move {
    let outcome = transmit(&ctx, remote_addr, bundle).await; // -> TransferOutcome
    ctx.sink.transfer_outcome(&bundle_id, outcome).await;
});
```

### File-CLA: Simple Batch Transfer

**Location**: `file-cla/` crate
**Pattern**: Filesystem-based transport with:
- Directory monitoring for inbound bundles
- File-based outbound bundle delivery
- Unidirectional and batch transfer patterns
- Simple peer discovery via directory structure

**Use Case**: demonstrates a minimal CLA implementation.

### BIBE: Protocol Layering

**Location**: `bibe/` crate
**Pattern**: Bundle-in-Bundle Encapsulation (RFC 9171 Appendix A.2):
- Encapsulation/decapsulation logic
- Tunneling through intermediate nodes
- Protocol layering concepts

## 4. Data Flow and Integration Points

### Bundle Processing Flow

```
Inbound:  CLA → Sink::dispatch() → BPA Dispatcher → Storage/Services
Outbound: BPA Dispatcher → CLA::forward() → Network/Transport
Routing:  TVR → RoutingSink → BPA RIB → Forwarding Decisions
```

### Detailed Integration Sequence

1. **Inbound Bundle Processing**:
   - The CLA receives a bundle from its network transport.
   - It calls `sink.dispatch()` with the bundle stream and peer context, and takes the returned `Acceptance` verdict to decide whether to acknowledge the transfer to its peer.
   - The BPA `Dispatcher` processes the bundle (validation, routing, storage).

2. **Outbound Bundle Processing**:
   - The BPA `Dispatcher` queries the `Rib` for a routing decision (populated by TVR and by CLA-reported peers).
   - It selects the appropriate CLA and peer for forwarding.
   - It calls `cla.forward()` with the bundle stream, lane, and destination address.
   - The CLA handles transport-specific delivery and returns a `ForwardBundleResult`.

3. **Peer Management**:
   - CLAs discover network-level peers via their transport mechanisms.
   - They report them via `sink.add_peer()` / `sink.remove_peer()` (both return `Result<bool>`: `true` when the peer set actually changed).
   - The BPA updates its peer table (`bpa/src/cla/peers.rs`) and installs or removes the corresponding routing entries.

### TVR ↔ CLA Coordination

- **Decoupled Operation**: TVR manages routing-level topology through time-based contact plans.
- **CLA Independence**: CLAs handle transport-level peer discovery and data transfer.
- **Shared State**: both systems update the BPA's routing information independently — TVR via `RoutingSink`, CLAs via `Sink::add_peer`/`remove_peer`.
- **No Direct Communication**: TVR and CLAs interact only through BPA interfaces; they never talk to each other directly.

## 5. Peer Management and Flow Control

### PeerTable Architecture

**Location**: `bpa/src/cla/peers.rs`

Each peer (`Peer`) holds:
- A `Vec<storage::channel::Sender>` of per-queue egress channels — one poller per policy queue, indexed by queue index, with queue 0 always present (`FlowControllerFactory::queue_count` is non-zero).
- A `controller: Arc<dyn policy::FlowController>` that owns the queue assignment, so the hot forwarding path touches no shared policy state.

Peers are minted via a reservation (`PeerTable::reserve`) and published only after full construction (`Reservation::publish`), so a reachable `Peer` is always a working `Peer`.

**Flow Control Pattern** (abridged from `Peer::forward`, illustrative but faithful to the real shape):

```rust
// Peer::forward -> core::result::Result<(), bundle::Bundle>
pub async fn forward(&self, next_hop: Eid, bundle: bundle::Bundle)
    -> core::result::Result<(), bundle::Bundle>
{
    // The per-peer controller owns the queue assignment.
    let queue = self.controller.queue_for();
    // An out-of-range index is a policy bug: clamp to queue 0, which always exists.
    let queue = self.queues.get(queue as usize).unwrap_or_else(|| {
        warn!("Egress policy classified a bundle into out-of-range queue {queue}");
        &self.queues[0]
    });

    // Build the ForwardPending status (peer, queue, next_hop) and enqueue.
    let status = /* BundleStatus::ForwardPending { peer, queue, next_hop } */;
    match queue.send_to(bundle, status).await {
        Ok(_) => Ok(()),
        Err(storage::channel::SendError(b)) => Err(b),  // backpressure hands the bundle back
    }
}
```

Note the real contract: queue selection goes through `self.controller.queue_for()` with an out-of-range index clamped to queue 0 (not a raw unchecked index into `self.queues`), the send is `queue.send_to(bundle, status)`, and backpressure surfaces as `Err(storage::channel::SendError(b))`, which `forward` maps to `Err(b)` so the return type is `Result<(), bundle::Bundle>` — the bundle ownership is handed back to the caller. Queues are instantiated eagerly per declared lane, capped at 256 (`egress_queue.rs`).

### Transfer Outcome Reporting: the reliable/unreliable model

The two layers of the forwarding result — `ForwardBundleResult` returned by `forward`, and the later `TransferOutcome` reported through `Sink::transfer_outcome` — encode *who owns the transfer* when `forward` returns (`bpa/src/cla/mod.rs`):

```rust
pub enum ForwardBundleResult {
    Sent,         // unreliable CL: terminal, fire-and-forget; no later outcome
    Accepted,     // reliable CL: ownership taken, outcome reported later
    NoNeighbour,  // the neighbour is no longer available
}

pub enum TransferOutcome {
    Completed,    // handed to the far bundle node
    Failed,       // the CL gave up (NOT proof of non-delivery)
}
```

- `Sent` is the terminal answer of an **unreliable** convergence layer: fire-and-forget, with no later outcome to report.
- `Accepted` is for a **reliable** convergence layer that learns its peer's acceptance later. Returning `Accepted` early frees the egress lane while the acknowledgement is in flight, so a session can pipeline many transfers concurrently; a `forward` that awaited the final outcome would serialize every lane on a wire round-trip. The BPA retains the bundle until the outcome arrives (or the peer is removed), and the CLA reports the result via `Sink::transfer_outcome(bundle_id, outcome)`.
- `NoNeighbour` means the neighbour is gone.
- `TransferOutcome::Failed` is **not** proof of non-delivery: the far end may already hold the bundle (the acknowledgement was lost), and receiving-side deduplication absorbs any re-forward.

This is exactly the model an LTP-CLA uses for reliable (RED) transfers: answer `Accepted` from `forward`, run the LTP session to completion asynchronously, then report `Completed` or `Failed` via `transfer_outcome`.

## 6. LTP-CLA Implementation Guidelines

*All code in this section is proposed / illustrative — it does not exist in Hardy today and is written to be consistent with the real APIs corrected above.*

### Required Components

1. **Core CLA Implementation**:
   - Implement the `Cla` trait with LTP-specific logic.
   - Handle LTP engine management and session establishment.
   - Manage reliability classes (RED/GREEN data).

2. **Address System Extension**:
   - Preferred: encode the LTP engine identifier inside the existing `ClaAddress::Private(Bytes)` variant (no core change — see section 2).
   - Alternative: add proposed `ClaAddressType::Ltp` / `ClaAddress::Ltp(LtpEngineId)` variants to the core enums.

3. **LTP Protocol Integration**:
   - LTP session management and engine configuration.
   - Segment transmission and acknowledgments.
   - Flow control per reliability class.

### Integration Patterns to Follow

**Registration Pattern** (proposed / illustrative, following the TCPCLv4 shape):

```rust
impl Cla for LtpCla {
    async fn on_register(&self, sink: Box<dyn Sink>, node_ids: &[NodeId], max_bundle_size: Option<NonZeroU64>) {
        // Store the sink for the CLA's lifetime.
        self.inner.call_once(|| Inner {
            sink: sink.into(),
            node_ids: node_ids.into(),
            max_bundle_size,
        });

        // Start LTP engines and listeners.
        self.start_ltp_services();
    }
}
```

**Peer Discovery Pattern** (proposed / illustrative — `add_peer` returns `Result<bool>`; `ClaAddress::Ltp` is the proposed-addressing form, or use `ClaAddress::Private`):

```rust
async fn on_ltp_session_start(&self, remote_engine_id: LtpEngineId, remote_eid: Option<NodeId>) {
    let cla_addr = ClaAddress::Ltp(remote_engine_id); // proposed; or ClaAddress::Private(encode(remote_engine_id))
    let node_ids = remote_eid.map_or(vec![], |eid| vec![eid]);

    if let Some(sink) = self.get_sink() {
        let _changed: bool = sink.add_peer(cla_addr, &node_ids).await?;
    }
}
```

**Reliable Transfer Pattern** (proposed / illustrative — matches the real `forward` signature and the `Accepted` + `transfer_outcome` contract):

```rust
async fn forward(
    &self,
    lane: Option<u32>,
    cla_addr: &ClaAddress,
    bundle_id: &Id,
    total_len: u64,
    stream: &mut dyn Receiver<Segment>,
) -> Result<ForwardBundleResult> {
    let ClaAddress::Ltp(engine_id) = cla_addr else { // proposed addressing
        return Ok(ForwardBundleResult::NoNeighbour);
    };

    // Buffer the bundle for LTP transmission.
    let bundle = stream::buffer_stream(stream, total_len).await?;

    // Determine reliability class based on bundle or policy.
    let reliability_class = self.classify_bundle(&bundle);

    // Start LTP transmission asynchronously, then report the outcome later.
    let sink = self.get_sink()?;
    let bundle_id = bundle_id.clone();
    spawn!(async move {
        let outcome = match ltp_transmit(engine_id, bundle, reliability_class).await {
            Ok(_)  => TransferOutcome::Completed,
            Err(_) => TransferOutcome::Failed,
        };
        sink.transfer_outcome(&bundle_id, outcome).await;
    });

    Ok(ForwardBundleResult::Accepted)
}
```

### Recommended Architecture

1. **LTP Engine Management**: use Hardy's `TaskPool` for managing LTP engines.
2. **Session Handling**: follow TCPCLv4's connection-registry pattern.
3. **Flow Control**: leverage the existing peer table and egress-queue infrastructure (section 5), mapping reliability classes onto lanes.
4. **Configuration**: use Hardy's configuration patterns for LTP engine configuration.
5. **Testing**: follow Hardy's test patterns with temporary directories and mock peers.

### Registering the LTP-CLA

There is no `BpaBuilder::with_ltp_cla` helper and no `self.clas.push(...)` registration API. A CLA registers through `BpaRegistration::register_cla`, after the BPA has been started:

```rust
// In-process: `bpa` implements BpaRegistration.
let node_ids = bpa
    .register_cla(
        "ltp".to_string(),
        ltp_cla.clone(),
        None,                 // optional egress FlowControllerFactory
        ltp_cla.cla_init(),   // illustrative accessor returning ClaInit
    )
    .await?;
```

The call drives `Cla::on_register` to completion and returns the BPA's node IDs. For an out-of-process CLA attached over gRPC, use `hardy_proto::client::RemoteBpa`, which also implements `BpaRegistration::register_cla` — the same mechanism TVR uses for `register_routing_agent` in `tvr/src/main.rs` (note that the egress policy is not carried over the gRPC path).

## 7. Performance and Scalability Considerations

### Memory Management
- Use `stream::buffer_stream()` judiciously for large bundles.
- Consider streaming segments directly to LTP for memory efficiency.
- Implement proper backpressure via bounded channels.

### Concurrency
- Follow Hardy's async patterns with `TaskPool` management.
- Use `CancellationToken` for graceful shutdown.
- Avoid blocking operations in async contexts.

### Resource Limits
- Implement per-peer transfer limits (following TCPCLv4's permit pattern).
- Configure appropriate LTP engine buffer sizes.
- Monitor and report resource usage via Hardy's metrics system.

## 8. Testing Strategy

### Unit Testing Patterns
- Mock `Sink` implementations for CLA testing.
- Temporary directories for LTP engine state.
- Deterministic test scenarios (following Hardy's test style guide).

### Integration Testing
- Multi-engine LTP scenarios.
- Bundle fragmentation and reassembly.
- Reliability class handling (RED vs GREEN data).
- Interoperability with existing CLAs.

## Conclusion

Hardy's architecture provides clear extension points for an LTP-CLA through well-defined traits and a clean separation of concerns. The existing TCPCLv4 implementation is a good reference for complex protocol CLAs, while the TVR system operates independently and works with any transport once the CLA interfaces are implemented.

The key success factors for an LTP-CLA implementation are:
1. Follow Hardy's async patterns and the Sink lifecycle (store the sink; dropping it unregisters).
2. Correctly implement the `Cla` and `Sink` trait interfaces, including the `Acceptance` verdict on `dispatch`.
3. Leverage the existing peer-management and flow-control (lane) infrastructure.
4. Handle deferred transfer outcomes (`Accepted` + `transfer_outcome`) for reliable delivery.
5. Integrate LTP addressing via `Private(Bytes)` or a proposed `Ltp` variant.

---

*Generated from Hardy DTN implementation analysis - commit 52ad76b2 ("Merge pull request #756 from EmbryoSpace/oci-bpa-server-file-cla")*
