# StellarFlow Ledger Rent Model

This guide documents expected storage rent costs for every pool, balance key, and transaction invocation across the StellarFlow smart contracts. All costs are expressed in **stroops** (1 XLM = 10,000,000 stroops). Ledger estimates assume a 5-second block time on Stellar Mainnet.

---

## Table of Contents

1. [Soroban Storage Tiers Overview](#1-soroban-storage-tiers-overview)
2. [Storage Cost Formulas](#2-storage-cost-formulas)
3. [Per-Contract Storage Key Catalog](#3-per-contract-storage-key-catalog)
   - [Price Oracle](#31-price-oracle)
   - [Root TimeLockedUpgradeContract (Main Oracle + Staking)](#32-root-timelockedupgradecontract-main-oracle--staking)
   - [Gas Tank](#33-gas-tank)
   - [Liquidity Lock](#34-liquidity-lock)
   - [Reward Splitter](#35-reward-splitter)
4. [TTL Constants Reference](#4-ttl-constants-reference)
5. [Transaction Invocation Costs](#5-transaction-invocation-costs)
6. [Calculation Examples — Liquidity Providers](#6-calculation-examples--liquidity-providers)
7. [Calculation Examples — Traders](#7-calculation-examples--traders)
8. [Rent Optimization Notes](#8-rent-optimization-notes)

---

## 1. Soroban Storage Tiers Overview

Soroban offers three storage tiers with different rent characteristics:

| Tier | Purpose in StellarFlow | Auto-purge | Relative Cost |
|------|------------------------|------------|---------------|
| **Instance** | Contract config, signers, fee pools, totals | No — lives with contract instance | Lowest per-byte; shared across all callers |
| **Persistent** | Node profiles, stakes, gas-tank balances, subscriptions | No — survives indefinitely when rent is paid | Medium; each key has its own TTL |
| **Temporary** | Price data, TWAP buffers, voting ballots, heartbeats | Yes — auto-purged after TTL expires | Cheapest for write-heavy, ephemeral data |

> **Key insight:** Price data (the highest-frequency write path) was intentionally moved to temporary storage in issue #127 to minimize ongoing rent for relayers and oracle consumers.

---

## 2. Storage Cost Formulas

### 2.1 Rent Calculation Basis

Soroban charges rent as a function of **entry size** and **time-to-live (TTL)**:

```
rent_fee = entry_size_bytes × fee_per_byte_per_ledger × ttl_ledgers
```

- `entry_size_bytes` — serialized XDR size of the stored value plus the key overhead (~100 bytes base).
- `fee_per_byte_per_ledger` — network-level parameter (currently ~0.000_013_67 stroops/byte/ledger on Mainnet).
- `ttl_ledgers` — the requested TTL window for the entry.

### 2.2 TTL Extension Formula

When a storage entry's remaining TTL falls below the threshold, StellarFlow automatically bumps it:

```
cost_to_extend = entry_size_bytes × fee_per_byte_per_ledger × (extend_to - current_ttl)
```

Where `extend_to` and the bump trigger `threshold` are the constants defined in each module (see [§4](#4-ttl-constants-reference)).

### 2.3 Approximate Byte Sizes per Entry Type

These are XDR-serialized estimates. Actual sizes depend on address lengths (56 bytes for a Stellar address).

| Entry Type | Approx. Size (bytes) | Notes |
|------------|---------------------|-------|
| `i128` balance | 24 | Key + u128 value |
| `Address` (key component) | 56 | Ed25519 public key |
| `PriceData` struct | ~220 | price + timestamp + ledger_seq + provider + decimals + confidence + ttl |
| `PriceBufferEntry` | ~100 | price + provider + timestamp |
| `PriceBuffer` (10 entries) | ~1 100 | Vec of 10 PriceBufferEntry + metadata |
| `StreamData` | ~48 | start_ledger + total_amount + claimed_amount |
| `NodeProfile` | ~80 | node address + rate + confidence + updated_at |
| `FeedStakeRecord` | ~100 | node + asset_id + amount + tier + registered_at |
| `CorridorFeePool` | ~28 | asset_id + collected + variable_pool |
| `VotingBallot` (1 vote) | ~200 | target + replacement + proposer + proposed_at + votes map |
| `Vec<Address>` (N entries) | ~56N + 16 | RelayerFunders list |

### 2.4 Fixed-Point Scale Reference

Fee math uses the following scales throughout the codebase:

```
STANDARD_FIXED_POINT_SCALE  = 10_000_000   (10^7)
INTERIOR_FEE_PRECISION_SCALE = 100_000_000_000_000  (10^14)
```

Multi-hop fee shares are computed at 10^14 precision then normalized back to 10^7 before ledger writes, preventing rounding loss across relayer splits.

---

## 3. Per-Contract Storage Key Catalog

### 3.1 Price Oracle

Contract path: `contracts/price-oracle/`

| Storage Key | Tier | TTL / Bump | Approx. Entry Size | Notes |
|-------------|------|-----------|---------------------|-------|
| `Admin` | Instance | With contract | ~56 bytes | Single admin address |
| `PendingAdmin` | Instance | With contract | ~56 bytes | Pending two-step transfer |
| `Initialized` / `IsLocked` / `Destroyed` | Instance | With contract | ~5 bytes each | Boolean flags |
| `QueryFee` | Instance | With contract | ~16 bytes | Fee in stroops |
| `FeeToken` / `SlashToken` / `InsuranceReserve` | Instance | With contract | ~56 bytes each | SEP-41 token addresses |
| `MaxPriceDeviationBps` | Instance | With contract | ~8 bytes | u32 |
| `WeightThreshold` | Instance | With contract | ~8 bytes | u32 |
| `MinQuorumThreshold` | Instance | With contract | ~8 bytes | u32 |
| `GasTank` | Instance | With contract | ~56 bytes | Gas-tank contract address |
| `HealthActiveRelayers` / `HealthPaused` / `HealthTotalAssets` / `HealthLastLedger` | Instance | With contract | ~8 bytes each | Isolated health counters |
| `VerifiedPrice(Symbol)` | Persistent | threshold=267,840 / extend=535,680 (~30 days) | ~220 bytes | One entry per tracked asset; extended during every `get_price` call |
| `AssetInfo(Symbol)` | Persistent | threshold=267,840 / extend=535,680 | ~80 bytes | Name + decimals; extended during `get_price` |
| `Twap(Symbol)` | Persistent | threshold=267,840 / extend=535,680 | ~1 100 bytes | Rolling 10-entry buffer; extended during `get_price` |
| `PriceBoundsEntry(Symbol)` | Persistent | With `VerifiedPrice` | ~40 bytes | min/max price per asset |
| `PriceFloorEntry(Symbol)` | Persistent | With `VerifiedPrice` | ~20 bytes | Absolute floor price |
| `PrevPriceBoundsEntry(Symbol)` / `PrevPriceFloorEntry(Symbol)` / `PrevMaxDeviationBps` | Persistent | With entry | ~40 bytes | Rollback slots written before each update |
| `AssetMeta(Symbol)` | Persistent | With `AssetInfo` | ~16 bytes | base_decimals + quote_decimals |
| `ProviderStake(Address)` | Persistent | Admin-managed | ~24 bytes | i128 collateral in stroops |
| `ProviderRewardBalance(Address)` | Persistent | Admin-managed | ~24 bytes | Accumulated reward |
| `ProviderConsecutiveMissedBlocks(Address)` | Persistent | Admin-managed | ~8 bytes | u32 miss counter |
| `ProviderLastSeenLedger(Address)` | Persistent | Admin-managed | ~8 bytes | u32 ledger sequence |
| `ProviderUptimeStreakStart(Address)` | Persistent | Admin-managed | ~8 bytes | u64 timestamp |
| `ProviderLastDeviationBps(Address)` | Persistent | Admin-managed | ~8 bytes | u32 bps deviation |
| `LiquidityThreshold(Symbol)` | Persistent | Admin-managed | ~24 bytes | i128 minimum liquidity |
| `ProviderReportedLiquidity(Address, Symbol)` | Persistent | Admin-managed | ~24 bytes | Last reported value |
| `LastLiquidityValidation(Symbol)` | Persistent | Admin-managed | ~8 bytes | u64 timestamp |
| `CorridorFeeVaultBalance(Address)` | Persistent | Admin-managed | ~24 bytes | i128 fee vault per token |
| `AdminWeight(Address)` | Persistent | Admin-managed | ~8 bytes | u32 governance weight |
| `CommunityPrice(Symbol)` | Temporary | Auto-purge (~TTL set per call) | ~220 bytes | Community-submitted price; never used in internal math |
| `PriceBufferByAsset(Symbol, u64)` | Temporary | Auto-purge (~TTL set per call) | ~1 100 bytes | Per-(asset, ledger_sequence) buffer |
| `Rewards` | Persistent | Admin-managed | ~varies | Legacy reward map |
| `RecentEvents` | Persistent | Admin-managed | ~varies | Dashboard event feed |
| `BaseCurrencyPairs` | Instance | With contract | ~varies | Registered asset list |

### 3.2 Root TimeLockedUpgradeContract (Main Oracle + Staking)

Contract path: `src/` (root crate)

| Storage Key | Tier | TTL / Bump | Approx. Entry Size | Notes |
|-------------|------|-----------|---------------------|-------|
| `DATA_KEY` ("DATA") | Instance | With contract | ~80 bytes | ContractData: admin address + value |
| `SIGNERS_KEY` ("SIGNERS") | Instance | With contract | ~8 bytes | u32 signer count |
| `PENDING_UPGRADE_KEY` ("PENDING") | Instance | With contract | ~96 bytes | StagedUpgrade: wasm hash + proposer + staged_at |
| `STAKE_REGISTRY_KEY` ("STAKES") | Instance | With contract | ~(N×80)+16 bytes | Map<Address, u64> — grows with validator count |
| `HEARTBEAT_KEY` ("HBEAT") | Temporary | Auto-purge | ~(N×12)+16 bytes | Map<AssetId, u64> timestamps |
| `NODE_PROFILES_KEY` ("NODES") | Persistent | threshold=10,000 / extend=`max_ttl` | ~(N×80)+16 bytes | Map<Address, NodeProfile> |
| `PLATFORM_CAPITAL_KEY` ("CAPITAL") | Instance | With contract | ~8 bytes | u64 total capital |
| `TREASURY_KEY` ("TREASURY") | Instance | With contract | ~56 bytes | Address |
| `SLASHED_STAKES_KEY` ("SLASHED") | Instance | With contract | ~(N×80)+16 bytes | Map<Address, u64> |
| `PRICE_VARIANCE_CONFIG_KEY` ("PVARCFG") | Instance | With contract | ~40 bytes | PriceVarianceConfig struct |
| `StakeKey(Address)` | Instance | With contract | ~24 bytes | u64 stake amount per node |
| `SignerKey(Address)` | Instance | With contract | ~5 bytes | unit (existence flag) |
| `RevokedSignerKey(Address)` | Instance | With contract | ~5 bytes | unit (existence flag) |
| `HeartbeatKey(u32)` | Temporary | threshold=5,000 / extend=100,000 | ~16 bytes | Last update timestamp per asset |
| `NodeProfileKey(Address)` | Persistent | threshold=10,000 / extend=max_ttl | ~80 bytes | Individual node profile |
| `FeedStakeKey(Address, Symbol)` | Persistent | Admin-managed | ~100 bytes | Per-node per-feed stake record |
| `AssetMetricsKey(Symbol)` | Persistent | Admin-managed | ~16 bytes | AssetFeedMetrics: volume_score + volatility_bps |
| `CorridorFeeKey(Symbol)` | Persistent | Admin-managed | ~28 bytes | CorridorFeePool |
| `DataKey::Subscription(Address)` | Persistent | threshold=259,200 / extend=518,400 (~30 days) | ~5 bytes | Consumer subscription flag |
| `DataKey::AssetPrice(Symbol)` | Persistent | threshold=5,000 / extend=100,000 | ~220 bytes | Asset price slot |
| `FeesStorageKey::CorridorPool(AssetId)` | Instance | With contract | ~28 bytes | Corridor fee accumulator |
| `CorridorWeightKey::Profile(AssetId)` | Instance | With contract | ~28 bytes | Corridor weight profile |
| `StakingStorageKey::TierConfig` | Instance | With contract | ~24 bytes | regional/standard/premier min stakes |
| `StakingStorageKey::AssetMetrics(AssetId)` | Persistent | Admin-managed | ~16 bytes | Volume + volatility scores |
| `StakingStorageKey::FeedStake(Address, AssetId)` | Persistent | Admin-managed | ~100 bytes | Feed-level stake record |
| `BallotKey::Proposal(Symbol)` | Temporary | threshold=5,000 / extend=17,280 (~1 day) | ~200+ bytes | VotingBallot; grows with each vote |
| `EMERGENCY_REVOCATION_TEMP_KEY` ("EMREV_T") | Temporary | Default=172,800 / extended=259,200 | ~200+ bytes | Emergency revocation proposal |
| `REVOCATION_TEMP_KEY` ("REVOK_T") | Temporary | Default=172,800 / extended=259,200 | ~200+ bytes | Standard revocation proposal |

### 3.3 Gas Tank

Contract path: `contracts/gas-tank/`

| Storage Key | Tier | TTL / Bump | Approx. Entry Size | Notes |
|-------------|------|-----------|---------------------|-------|
| `DataKey::Token` | Instance | With contract | ~56 bytes | SEP-41 token address |
| `DataKey::Oracle` | Instance | With contract | ~56 bytes | Authorized oracle address |
| `DataKey::Balance(Address)` | Persistent | Admin/caller must extend | ~24 bytes | Consumer gas balance in stroops |
| `DataKey::Allowance(Address, Address)` | Persistent | Admin/caller must extend | ~24 bytes | Per-(consumer, relayer) spending cap |
| `DataKey::RelayerFunders(Address)` | Persistent | Admin/caller must extend | ~(N×56)+16 bytes | List of consumers funding a relayer; grows with N funders |

> **Note:** Gas-tank persistent entries have no automatic TTL bump. Callers must include rent bumps in `deposit` / `set_allowance` transactions if entries risk expiry.

### 3.4 Liquidity Lock

Contract path: `contracts/liquidity-lock/`

| Storage Key | Tier | TTL / Bump | Approx. Entry Size | Notes |
|-------------|------|-----------|---------------------|-------|
| `DataKey::Admin` | Instance | With contract | ~56 bytes | Admin address |
| `DataKey::Token` | Instance | With contract | ~56 bytes | SEP-41 token address |
| `DataKey::Stream(Address)` | Instance | With contract | ~48 bytes | StreamData per recipient; streams expire with the contract instance |

All stream data lives in instance storage, so its rent is bundled with the contract instance TTL. The 3,000-ledger linear vesting schedule (~4.2 hours) is enforced in contract logic, not by storage TTL.

### 3.5 Reward Splitter

Contract path: `contracts/reward-splitter/`

| Storage Key | Tier | TTL / Bump | Approx. Entry Size | Notes |
|-------------|------|-----------|---------------------|-------|
| Instance config (admin, token, etc.) | Instance | With contract | ~80 bytes | Standard initialization config |
| Recipient shares | Persistent | Admin-managed | ~(N×80)+16 bytes | Map of address → share weight |

Cooldown windows enforced by the reward splitter: stage 1 = 3,600 s, stage 2 = 28,800 s, stage 3 = 86,400 s.

---

## 4. TTL Constants Reference

| Constant | Value (ledgers) | Approx. Wall Time | Location | Storage Tier |
|----------|----------------|-------------------|----------|--------------|
| `PERSISTENT_BUMP_AMOUNT` | 535,680 | ~31 days | `contracts/price-oracle/src/storage.rs` | Persistent |
| `PERSISTENT_THRESHOLD` | 267,840 | ~15.5 days | `contracts/price-oracle/src/storage.rs` | Persistent |
| `RENT_EXTEND_TO` | 518,400 | ~30 days | `src/storage.rs` | Persistent |
| `RENT_THRESHOLD` | 259,200 | ~15 days | `src/storage.rs` | Persistent |
| `ASSET_TTL_EXTEND_TO` | 100,000 | ~5.8 days | `src/storage.rs` | Persistent |
| `ASSET_TTL_THRESHOLD` | 5,000 | ~7 hours | `src/storage.rs` | Persistent |
| `INSTANCE_TTL_EXTEND` | 100,000 | ~5.8 days | `src/lib.rs` | Instance |
| `RELAYER_TTL_THRESHOLD` | 5,000 | ~7 hours | `src/lib.rs` | Persistent |
| `PROFILE_TTL_THRESHOLD` | 10,000 | ~13.9 hours | `src/storage.rs` | Persistent |
| `BALLOT_TTL_LEDGERS` | 17,280 | ~24 hours | `src/governance.rs` | Temporary |
| `BALLOT_TTL_THRESHOLD` | 5,000 | ~7 hours | `src/governance.rs` | Temporary |
| `DEFAULT_PROPOSAL_TTL` | 172,800 | ~10 days | `src/temp_governance.rs` | Temporary |
| `EXTENDED_PROPOSAL_TTL` | 259,200 | ~15 days | `src/temp_governance.rs` | Temporary |
| `UPGRADE_DELAY_SECONDS` | 172,800 s | 48 hours | `src/lib.rs` | — (time-based, not storage) |

> All wall-time estimates assume 5-second ledger close times on Stellar Mainnet.

---

## 5. Transaction Invocation Costs

Each invocation pays for CPU instructions and any storage reads/writes/extends it touches. The table below lists the key operations and the storage mutations they trigger.

### 5.1 Price Oracle — Invocation Cost Breakdown

| Function | Storage Reads | Storage Writes / Extends | Approx. Additional Rent | Notes |
|----------|--------------|--------------------------|------------------------|-------|
| `initialize` | 0 | Instance: Admin, BaseCurrencyPairs, Initialized (~5 keys) | Negligible — instance shared cost | One-time deploy cost |
| `add_asset` | 1 instance read | Temporary: `VerifiedPrice`, `TrackedAsset`, `AssetInfo`, `HealthTotalAssets` | ~0.04 stroops/ledger × TTL | Creates 4 storage entries |
| `update_price` (relayer call) | 3–5 reads | Temporary: `VerifiedPrice(asset)`, `PriceBufferByAsset(asset, ledger)`, `Twap(asset)`, `HealthLastLedger` | ~0.06 stroops/ledger × TTL | Hot path; cheapest per-byte tier |
| `get_price` (consumer call) | 1 persistent read | Persistent extends: `VerifiedPrice`, `AssetInfo`, `Twap` (3 TTL bumps) | Bump cost = `entry_size × fee_per_byte × extend_delta` | Caller implicitly pays rent for data they consume |
| `set_price_bounds` | 1 persistent read | Persistent: `PriceBoundsEntry`, `PrevPriceBoundsEntry` | ~0.003 stroops/ledger × 535,680 | Rollback slot also written |
| `set_price_floor` | 1 persistent read | Persistent: `PriceFloorEntry`, `PrevPriceFloorEntry` | ~0.002 stroops/ledger × 535,680 | Rollback slot also written |
| `register_provider` | 1 read | Persistent: `ProviderStake`, `ProviderLastSeenLedger` | ~0.004 stroops/ledger × 518,400 | One entry per provider |
| `slash_provider` | 2 reads | Persistent update: `ProviderStake`, `ProviderLastDeviationBps` | Negligible (updates existing entries) | No new allocation |
| `query_oracle_health` | 4 instance reads | Instance extends: 4 health slots | Negligible — instance tier | Health slots are tiny |

### 5.2 Gas Tank — Invocation Cost Breakdown

| Function | Storage Reads | Storage Writes / Extends | Notes |
|----------|--------------|--------------------------|-------|
| `deposit(consumer, amount)` | 1 persistent read | 1 persistent write: `Balance(consumer)` | Creates entry if first deposit; ~24 bytes |
| `withdraw(consumer, amount)` | 1 persistent read | 1 persistent write: `Balance(consumer)` | Updates existing balance |
| `set_allowance(consumer, relayer, amount)` | 2 persistent reads | 2 persistent writes: `Allowance(consumer, relayer)`, `RelayerFunders(relayer)` | `RelayerFunders` grows by 56 bytes per new consumer added |
| `reimburse(relayer)` | N+1 persistent reads | N×2 persistent writes | N = number of funders; each write updates `Balance` + emits event |

### 5.3 Liquidity Lock — Invocation Cost Breakdown

| Function | Storage Reads | Storage Writes / Extends | Notes |
|----------|--------------|--------------------------|-------|
| `initialize` | 0 | Instance: Admin, Token | Negligible — shared instance cost |
| `create_stream(recipient, amount)` | 1 instance read | Instance write: `Stream(recipient)` | ~48 bytes; bundled with contract rent |
| `get_claimable(recipient)` | 1 instance read | None | Read-only; no storage mutation |
| `claim(recipient)` | 2 instance reads | Instance write: `Stream(recipient)` | Updates `claimed_amount` in-place |

### 5.4 Root Contract (Staking + Governance) — Invocation Cost Breakdown

| Function | Storage Reads | Storage Writes / Extends | Notes |
|----------|--------------|--------------------------|-------|
| `register_node` | 1 instance read | Instance: `StakeKey`, Persistent: `NodeProfileKey`, `FeedStakeKey` | ~180 bytes total new allocation |
| `stake(node, amount)` | 2 reads | Instance update: `StakeKey`; Persistent extend: `NodeProfileKey` | No new allocation if node already registered |
| `unstake(node, amount)` | 2 reads | Instance update: `StakeKey` | No new allocation |
| `propose_revocation` | 1 temp check | Temporary write: `REVOK_T` (~200 bytes, TTL=172,800) | Creates ballot; ~3 stroops initial rent |
| `vote_on_revocation` | 1 temp read | Temporary write (update + extend): `REVOK_T` | Ballot grows ~56 bytes per voter; TTL extended to 259,200 |
| `execute_revocation` | 1 temp read | Temporary remove: `REVOK_T`; Instance update: `RevokedSignerKey` | Ballot storage freed immediately |
| `stage_upgrade` | 1 instance read | Instance write: `PENDING` (~96 bytes) | 48-hour timelock enforced by timestamp |
| `execute_upgrade` | 1 instance read | Instance remove: `PENDING`; wasm replaced | No net new storage after upgrade |

---

## 6. Calculation Examples — Liquidity Providers

The examples below use the following baseline network rate:

```
fee_per_byte_per_ledger ≈ 0.000_013_67 stroops / byte / ledger
```

### Example A: New Liquidity Provider Registering a Node

**Action:** A new LP calls `register_node` then stakes 5,000 XLM against the KES/XLM feed.

**Storage allocated:**
- `StakeKey(provider)` — Instance, ~24 bytes
- `NodeProfileKey(provider)` — Persistent, ~80 bytes, extended to 518,400 ledgers
- `FeedStakeKey(provider, "KES")` — Persistent, ~100 bytes, extended to 518,400 ledgers

**Persistent rent calculation:**

```
NodeProfileKey:
  rent = 80 bytes × 0.000_013_67 × 518,400
       = 80 × 7.087 stroops
       ≈ 567 stroops  (~0.000_057 XLM)

FeedStakeKey:
  rent = 100 bytes × 0.000_013_67 × 518,400
       = 100 × 7.087 stroops
       ≈ 709 stroops  (~0.000_071 XLM)
```

**Total one-time persistent storage rent:** ~1,276 stroops (~0.000_128 XLM)

Instance storage cost is shared across all callers and is negligible per-invocation.

---

### Example B: Provider Submitting a Price Update

**Action:** A relayer calls `update_price("KES", price_value)` once per ledger.

**Temporary storage written (per call):**
- `VerifiedPrice("KES")` — Temporary, ~220 bytes
- `PriceBufferByAsset("KES", ledger_seq)` — Temporary, ~1,100 bytes (10-entry buffer)
- `Twap("KES")` extension — Persistent extend (~0 cost if TTL not below threshold)

**Temporary rent calculation** (TTL is per-entry, set to ~ASSET_TTL_EXTEND_TO = 100,000 ledgers):

```
VerifiedPrice:
  rent = 220 bytes × 0.000_013_67 × 100,000
       = 220 × 1.367 stroops
       ≈ 301 stroops  per write

PriceBufferByAsset:
  rent = 1,100 bytes × 0.000_013_67 × 100,000
       = 1,100 × 1.367 stroops
       ≈ 1,504 stroops  per write
```

**Per-update cost:** ~1,805 stroops (~0.000_181 XLM)

Since price data is temporary, it auto-purges after ~5.8 days without renewal cost. At one update per 5 seconds (~17,280 updates/day), daily write rent ≈ **17,280 × 1,805 ≈ 31,190,400 stroops (~3.12 XLM/day)** for a single asset feed. This cost is spread across the relayer set.

---

### Example C: Provider Earning and Claiming Rewards

**Action:** A provider accumulates rewards via fee distributions, then claims them.

**Storage touched:**
- `ProviderRewardBalance(provider)` — Persistent, ~24 bytes, TTL extended to 518,400 on update

**Rent for reward balance slot:**

```
rent = 24 bytes × 0.000_013_67 × 518,400
     = 24 × 7.087 stroops
     ≈ 170 stroops  (~0.000_017 XLM)
```

The slot persists as long as the provider is active and rent is paid via TTL bumps during normal operations.

---

### Example D: Governance — Proposing a Key Revocation

**Action:** An LP proposes an emergency revocation via the governance module.

**Temporary storage written:**
- `EMREV_T` key — Temporary, ~200 bytes, TTL = 172,800 ledgers (~10 days)

```
rent = 200 bytes × 0.000_013_67 × 172,800
     = 200 × 2.362 stroops
     ≈ 472 stroops  (~0.000_047 XLM)
```

Each subsequent vote extends TTL to 259,200 ledgers (~15 days). If the proposal expires without execution, the network auto-purges it — no cleanup transaction required.

---

## 7. Calculation Examples — Traders

### Example E: Trader Calling `get_price`

**Action:** A DeFi protocol calls `get_price("NGN")` to read the current NGN/XLM rate before a swap.

**Storage read + implicit TTL extensions:**
- Read `VerifiedPrice("NGN")` — Persistent
- Extend `VerifiedPrice("NGN")` TTL if below 267,840 ledgers → bump to 535,680
- Extend `AssetInfo("NGN")` TTL if below 267,840 → bump to 535,680
- Extend `Twap("NGN")` TTL if below 267,840 → bump to 535,680

If all three entries are near expiry and need a full bump of `535,680 - 267,840 = 267,840` additional ledgers:

```
VerifiedPrice extension:
  cost = 220 × 0.000_013_67 × 267,840 ≈ 804 stroops

AssetInfo extension:
  cost = 80 × 0.000_013_67 × 267,840 ≈ 293 stroops

Twap extension:
  cost = 1,100 × 0.000_013_67 × 267,840 ≈ 4,022 stroops
```

**Worst-case `get_price` rent contribution:** ~5,119 stroops (~0.000_512 XLM)

In practice, each `update_price` call from a relayer already extends these entries, so traders typically encounter a much smaller or zero additional bump cost.

---

### Example F: Trader Depositing into Gas Tank

**Action:** A relayer consumer deposits 10 XLM into the Gas Tank to pre-fund future oracle query fees.

**Storage written:**
- `Balance(consumer)` — Persistent, ~24 bytes

If this is the consumer's first deposit (new entry):

```
rent = 24 bytes × 0.000_013_67 × 518,400
     ≈ 170 stroops  (~0.000_017 XLM)
```

Subsequent deposits update the same 24-byte slot — storage rent is already paid; only the TTL may need extending.

---

### Example G: Trader Using a Subscribed Protocol

**Action:** A protocol contract is a price-update subscriber (`PriceUpdateSubscribers`). A trade triggers `get_price`, which calls the subscriber callback via `on_price_update`.

**Additional storage per trade:**
- `PriceUpdateSubscribers` list read — Instance (free read)
- Cross-contract callback invocation — standard Soroban instruction cost, no new storage allocation

No additional persistent or temporary storage entries are created. The subscriber model imposes only CPU (instruction) costs per callback, not additional rent.

---

### Example H: Trader Stake Check for Premium Pool Access

**Action:** A premium liquidity pool checks `ProviderStake(provider)` to gate access.

**Storage read:**
- `ProviderStake(provider)` — Persistent, ~24 bytes

Read is free (no rent charge for reads, only writes and extends). The entry remains alive as long as the provider's last TTL extension is within 518,400 ledgers from the current ledger.

If the TTL has lapsed, the entry will be purged and the provider must re-register. The check `PremiumPoolAccessDenied` (error #23) is returned.

---

## 8. Rent Optimization Notes

### 8.1 Why Price Data Uses Temporary Storage

Oracle prices (the most frequently written data in StellarFlow) were migrated to temporary storage in issue #127. The reasoning:

- **Prices are time-sensitive:** stale prices are re-pushed by relayers on every ledger anyway.
- **Temporary is cheaper per-write:** no ongoing rent accumulation between updates.
- **TTL auto-purges stale data:** no manual cleanup transactions.
- **No API change required:** all public interfaces remain identical.

### 8.2 Composite Keys Reduce Cross-Asset Reads

Legacy single-map keys (`PriceBuffer`, `PriceBoundsData`, `PriceFloorData`) loaded all assets' data in one storage read, billing for the entire serialized map. They were replaced with composite per-asset keys:

- `PriceBufferByAsset(Symbol, u64)` — one slot per (asset, ledger)
- `PriceBoundsEntry(Symbol)` — one slot per asset
- `PriceFloorEntry(Symbol)` — one slot per asset

This means `get_price("NGN")` no longer loads KES, GHS, or ZAR bounds into memory.

### 8.3 Instance vs Persistent vs Temporary Decision Tree

```
Is the data needed on every contract call?
  → Yes: Instance storage (fee pools, admin, config)

Is the data long-lived but only accessed occasionally?
  → Yes: Persistent storage with TTL bump on access
  → Examples: node profiles, stakes, gas-tank balances

Is the data ephemeral or re-creatable?
  → Yes: Temporary storage (prices, heartbeats, voting ballots)
```

### 8.4 Staking Tier Impact on Storage Costs

The staking tier system (`Regional`, `Standard`, `Premier`) does not directly affect storage costs. It affects the minimum stake that must be held in `FeedStakeKey` entries. Higher tiers require larger stake balances but do not create additional storage slots.

Default minimum stakes:
- Regional: 100 units
- Standard: 1,000 units
- Premier: 10,000 units

### 8.5 Relayer Funder List Growth

The `RelayerFunders(relayer)` key in the Gas Tank stores a `Vec<Address>`. Each new consumer who funds a relayer appends 56 bytes. For a relayer with 100 consumers:

```
size = 56 × 100 + 16 (Vec overhead) = 5,616 bytes

30-day persistent rent = 5,616 × 0.000_013_67 × 518,400
                       ≈ 39,794 stroops (~0.003_979 XLM/month)
```

Protocols maintaining large funder lists should budget accordingly and extend TTL proactively via `set_allowance` calls.

---

*Last updated: July 2026. Network fee parameters are subject to change via Stellar protocol upgrades. Verify current base fees with `soroban network status` or the Stellar Horizon API.*
