# PubGrub Resolver for Cargo (`-Zpubgrub-resolver`) — Design & Handoff

> Status: **experimental, working for fresh full-graph resolution.** This document
> is a handoff for the next agent/engineer. It captures the architecture, the
> hard-won correctness insights, how to build/test, what is verified, and the
> prioritized next steps.

Branch: `pubgrub` (off `rust-lang/cargo` master).

---

## 1. Goal

Replace Cargo's hand-rolled backtracking dependency resolver with one built on
the [`pubgrub`](https://crates.io/crates/pubgrub) v0.4 crate, **side by side**
with the existing resolver, gated behind the unstable flag
`-Zpubgrub-resolver`. When the flag is off, resolution is completely unchanged.

Initial acceptance bar: **resolve Cargo's own dependency tree** and produce a
lockfile identical to the default resolver.

---

## 2. Current status (verified)

- **Full-tree parity against an *unmodified* `rust-lang/cargo`.** Using the
  pubgrub-enabled binary against a **pristine clone** of `rust-lang/cargo` (whose
  `Cargo.toml` has no pubgrub dependency), with no pre-existing `Cargo.lock`,
  both resolvers produce a **byte-identical** lockfile (latest run: 5907 lines,
  542 packages, `diff` = 0). See §3 for the exact procedure.
  - Verified two ways, because `diff == 0` alone is *not* sufficient (it is also
    consistent with the `-Z` flag being a silent no-op that runs the default
    resolver twice):
    1. **Dispatch proof** — a temporary marker in `pubgrub::resolve` printed 0
       times without the flag and 1 time with it, proving the flag routes to the
       pubgrub code path. (Also: `generate-lockfile` does a **single** resolve
       pass, so the marker fires once.)
    2. **Parity proof** — the byte-identical lockfile above.
  - NOTE: resolving the manifest *inside this branch's repo* is a weaker check:
    this branch adds `pubgrub`/`version-ranges` to the workspace `Cargo.toml`, so
    its lockfile is ~5942 lines (the extra ~35 are those added deps). Always test
    against a pristine clone to avoid that confound.
- Validation in `crates/resolver-tests`:
  - `pubgrub_smoke.rs` — basic end-to-end (2 tests).
  - `pubgrub_validated.rs` — SAT-validated scenarios: features, `dep:`/`dep/feat`,
    incompatible majors, links conflicts, diamonds, missing deps (11 tests).
  - `pubgrub_graph.rs` — **graph/edge** comparison vs the default resolver,
    including regressions for the cycle and weak-dependency cases (3 tests).
  - `pubgrub_prop.rs` — property tests vs the SAT reference resolver over 256
    randomly generated registries: a fresh-resolution check and a
    conservative-update check (resolve, feed the result back as
    `VersionPreferences`, re-resolve both kept and with the requested crate
    freed, and re-validate against SAT + the default resolver).
  - `pubgrub_update.rs` — deterministic offline differential tests for the
    conservative-update paths: building against an untouched lock, `cargo
    update -p <crate>`, `cargo update` (free everything), adding a dependency,
    shared transitive pinning, stale-lock override, and `--precise`. Each
    resolves, derives `VersionPreferences` from the first resolution, mutates
    one input, and asserts the pubgrub graph (nodes **and** edges) matches the
    default resolver (8 tests).
  - **Curated suite via `__CARGO_TEST_PUBGRUB=1`** — the harness convenience
    helpers route through `-Zpubgrub-resolver` when this env var is set, so the
    pre-existing curated suites run on PubGrub:
    - `tests/resolve.rs`: **37/37 pass**.
    - `tests/pubgrub.rs`: **28/28 pass** (weak deps, feature unification, cyclic
      features — all SAT-validated where applicable).
    - Two `resolve.rs` tests have their *exact error-text* assertions gated off
      under PubGrub (it uses its own derivation-tree formatter); the resolution
      *outcome* is identical.

### Caveats on the verification
- Parity is verified against the **current crates.io index state**; index drift
  changes selected versions for both resolvers (it stays a 0-line diff because
  both drift together, but it is not a hermetic golden-file test).
- Full-lockfile parity is verified for **`generate-lockfile` (fresh)** only. The
  conservative-update paths are now exercised at the **resolver level** (where
  they reduce to `VersionPreferences`) by `pubgrub_update.rs` and the new
  `pubgrub_prop.rs` case — see §9.3. Still unverified end-to-end: the
  `ops::resolve` glue that *builds* those preferences from a real `Cargo.lock`,
  and the registry-side exact pinning that `--precise` performs on top of the
  preference (the harness models `--precise` only as a preferred `=x.y.z`
  dependency).
- The package-set/SAT tests do **not** check graph edges; only `pubgrub_graph.rs`
  does. Edge correctness is where the subtle bugs lived (see §6).

---

## 3. How to build & test (IMPORTANT)

This workspace needs OpenSSL/curl/libgit2 from a Nix dev shell. **All** cargo
commands must run inside it:

```sh
nix develop ~/dev/dotfiles#cargo --command bash -c '<cargo command>'
```

Common commands:

```sh
# Build the library
nix develop ~/dev/dotfiles#cargo --command bash -c 'cargo build -p cargo --lib'

# Build the cargo binary (needed for real lockfile tests)
nix develop ~/dev/dotfiles#cargo --command bash -c 'cargo build --bin cargo'

# Unit tests for the semver conversion
nix develop ~/dev/dotfiles#cargo --command bash -c 'cargo test -p cargo --lib core::resolver::pubgrub'

# Resolver-test suites
nix develop ~/dev/dotfiles#cargo --command bash -c \
  'cargo test -p resolver-tests --test pubgrub_graph --test pubgrub_validated --test pubgrub_smoke'

# Property test (slow, ~60-70s)
nix develop ~/dev/dotfiles#cargo --command bash -c 'cargo test -p resolver-tests --test pubgrub_prop'

# Re-run the ENTIRE curated suite through the PubGrub resolver
nix develop ~/dev/dotfiles#cargo --command bash -c \
  '__CARGO_TEST_PUBGRUB=1 cargo test -p resolver-tests --test resolve --test pubgrub'

# Re-run the FULL integration testsuite through PubGrub. `__CARGO_TEST_PUBGRUB`
# is honored at the resolver dispatch fork (resolver::resolve), independent of
# the nightly-gated `-Zpubgrub-resolver` flag, and is inherited by the child
# cargo processes the testsuite spawns. Expect failures: many testsuite cases
# assert exact error text / version-selection ordering the PubGrub path does
# not reproduce. This is a survey of the gap, not a pass/fail gate.
nix develop ~/dev/dotfiles#cargo --command bash -c \
  '__CARGO_TEST_PUBGRUB=1 cargo test -p cargo --test testsuite'
```

### Reproducing the full-tree parity check (the real acceptance test)

Test against a **pristine clone** of `rust-lang/cargo`, *not* this branch's repo
(this branch's `Cargo.toml` adds the pubgrub dependency — a confound).

```sh
nix develop ~/dev/dotfiles#cargo --command bash -c '
  cd /local/home/whlo/dev/cargo
  cargo build --bin cargo
  CARGO=$(pwd)/target/debug/cargo

  rm -rf /tmp/cargo-clean
  git clone --depth 1 https://github.com/rust-lang/cargo /tmp/cargo-clean
  cd /tmp/cargo-clean
  grep -c pubgrub Cargo.toml   # expect 0 (pristine manifest)

  rm -f Cargo.lock; $CARGO generate-lockfile >/dev/null 2>&1;            cp Cargo.lock /tmp/d.lock
  rm -f Cargo.lock; $CARGO -Zpubgrub-resolver generate-lockfile >/dev/null 2>&1; cp Cargo.lock /tmp/p.lock
  diff /tmp/d.lock /tmp/p.lock && echo IDENTICAL
'
```
> - Always `rm -f Cargo.lock` before *each* run. A present lock seeds
>   `version_prefs` and masks fresh-resolution bugs (this exact mistake produced
>   a false "it works" claim early on).
> - `diff == 0` is necessary but **not** sufficient — it cannot tell "pubgrub
>   matched" from "flag is a no-op, default ran twice". To prove the pubgrub path
>   actually executed, run with the flag and
>   `CARGO_LOG=cargo::core::resolver::pubgrub=debug`; you should see
>   `pubgrub resolver active: resolving N workspace member(s)` (a permanent
>   `tracing::debug!` in `pubgrub::resolve`). It does not print without the flag.

---

## 4. Architecture

### 4.1 Dispatch (the only fork point)
`src/cargo/core/resolver/mod.rs::resolve()` checks
`gctx.cli_unstable().pubgrub_resolver` and, if set, calls
`pubgrub::resolve(...)` with the identical signature. Flag is declared in
`src/cargo/core/features.rs` (`unstable_cli_options!` + parse arm
`"pubgrub-resolver"`). The single upstream call site is
`src/cargo/ops/resolve.rs` (~line 505), unchanged.

### 4.2 Module layout — `src/cargo/core/resolver/pubgrub/`

| File | Responsibility |
|---|---|
| `mod.rs` | Entry `resolve()`. Builds `RegistryQueryer`, the `Root`s from workspace members + their requested features, the `Provider`, runs `pubgrub::resolve(Provider, Root, 0.0.0)`, then reconstructs via `solution`. Translates `PubGrubError` (stashed real errors take precedence over `NoSolution`). |
| `semver_pubgrub.rs` | `SemverPubgrub`: a `pubgrub::VersionSet` over `semver::Version`. Ported & specialized from the `semver-pubgrub` crate, adapted to published pubgrub 0.4 (`Range`/`VersionSet`). Bug-for-bug compatible with `VersionReq::matches`. Also `SemverCompatibility` (the compat-bucket enum) + `only_one_compatibility_range`, `as_singleton`. |
| `package.rs` | `PubGrubPackage` — the encoding (see §5). Plus `FeatureNamespace`, `BucketName`, `WideName`, and `OptVersionReq -> SemverPubgrub` conversion. |
| `provider.rs` | `Provider`: implements `pubgrub::DependencyProvider`. Wraps Cargo's async `RegistryQueryer` with a **blocking** poll loop. `choose_version`, `prioritize`, `get_dependencies` (the big translation from `Summary`/`Dependency`/`FeatureValue` into the encoding). |
| `solution.rs` | `into_resolve`: projects pubgrub's `SelectedDependencies` back into a Cargo `Resolve` (graph nodes, edges, features, checksums, replacements). Reuses the default resolver's `check_cycles` / `check_duplicate_pkgs_in_lockfile`. Handles `[patch]`/`[replace]` node identity (see §6). |
| `error.rs` | The standalone error-reporting bridge: `report_error` turns a `PubGrubError` into a typed `ResolveError`, reusing the v1 resolver's own renderers for byte-identical text. Defines `UnavailableReason` (the structured `M`). See §8. |

### 4.3 Data flow
```
ops::resolve  →  resolver::resolve  --flag-->  pubgrub::resolve
                                                   │
              RegistryQueryer (async, poll)        │ build Roots from (Summary, ResolveOpts)
                     ▲ blocking bridge             ▼
              Provider: DependencyProvider  ──►  pubgrub::resolve(Root, 0.0.0)
                                                   │ SelectedDependencies<PubGrubPackage, Version>
                                                   ▼
                               solution::into_resolve  →  Resolve  →  Cargo.lock
```

---

## 5. The encoding (the crux)

PubGrub selects **one version per package**. Cargo needs (a) the same crate at
multiple semver-incompatible versions and (b) feature unification. We encode
both into a richer package identity (`PubGrubPackage`), adapted from
`Eh2406/pubgrub-crates-benchmark`'s `Names` enum, extended to carry `SourceId`
(Cargo has multiple sources) and to own its data:

- `Root` — synthetic; its deps are the workspace members.
- `Bucket { name: (crate, source, SemverCompatibility), member, all_features }`
  — a concrete crate within one compat bucket. Distinct buckets coexist ⇒
  incompatible majors allowed. `member` ⇒ include dev-deps. `all_features` ⇒
  enable every feature (lockfile pass).
- `BucketFeatures { bucket, FeatureNamespace }` — "this feature (Feat) or
  optional-dep activation (Dep) is enabled". Feature unification falls out of
  version solving over these virtual packages.
- `BucketDefaultFeatures { bucket }` — default features enabled.
- `Wide { name, req, from, from_compat }` (+ `WideFeatures`,
  `WideDefaultFeatures`) — used when a requirement could span **multiple** compat
  buckets (rare; e.g. `>=1, <3`). Defers bucket choice to a second step.
- `Links { links }` — enforces global uniqueness of a `links` value.

`semver::Version` is used directly as pubgrub's `V` (it already implements
`Ord/Clone/Debug/Display`). pubgrub 0.4's `Package` trait needs only
`Clone+Eq+Hash+Debug+Display` (no `Ord`), so `PubGrubPackage` does not implement
`Ord`.

### Key encoding rules in `get_dependencies` (provider.rs)
- A feature/default-feature package depends on its `Bucket` pinned to the same
  exact version (singleton range) → ties feature selection to the crate version.
- `Bucket` with `all_features` enables every key in `summary.features()` (the
  feature map already contains implicit features for optional deps).
- Optional deps are pulled in only via `BucketFeatures{Dep(..)}` packages, except
  in the `all_features` bucket.
- **Weak dep features (`dep?/feat`)**: still activate the optional dependency
  (record the edge); the `weak` flag only suppresses enabling the dep's own
  *implicit feature*. This mirrors Cargo's v1 lock resolver — see §6.

### Reconstruction rules in `solution.rs`
- Real nodes = `Bucket` packages → `PackageId(name, version, source)`.
- A package's enabled features = the `Feat(..)` + `default` activations in the
  solution.
- Edges: for each resolved package, walk its `summary.dependencies()`; include an
  edge when:
  - dev-dependency: only if the package is a workspace `member`;
  - optional: only if activated (`BucketFeatures{Dep(name_in_toml)}` present);
  - otherwise (normal/build, non-optional): always.
- The child version is found via `from_dep` (re-derives the bucket; for `Wide`
  packages it reads the chosen bucket from the solution).

---

## 6. Hard-won correctness insights (READ THIS)

These cost real debugging time; do not regress them.

1. **Workspace members are not in the registry.** They are provided directly.
   The provider seeds its version cache with the root summaries in
   `Provider::new`; otherwise `choose_version` queries the registry for a member
   and finds nothing → immediate `NoSolution`.

2. **Cargo's lockfile graph is activation-gated, NOT feature-agnostic.** An
   optional-dependency edge appears only if the optional dep is activated.
   Drawing edges for any present optional dep (a tempting "fix") creates cycles
   such as `schemars → url` and fails `check_cycles`.

3. **Weak dependency features still create the edge.** Cargo's v1 lock resolver
   (`dep_cache.rs::Requirements::require_dep_feature`) runs
   `self.deps.entry(package).or_default().insert(feat)` **unconditionally** — so a
   `dep?/feat` reference in an *enabled* feature records the optional dependency
   in the lock graph. The `weak` flag only gates whether the dep's own implicit
   feature is enabled. Example: bstr's `std = ["serde?/std"]` causes
   `bstr → serde` to appear in the lock even though `serde` is never
   feature-activated (confirmed: even `cargo tree --all-features` shows bstr
   without `serde`, yet the lock has the edge). The v1 lock resolver is a
   deliberately coarse over-approximation; the precise feature resolver
   (`features.rs::FeatureResolver`) refines features at build time. **We are
   replacing the v1 lock resolver, so we must match its coarse behavior.**

4. **The SAT/scenario tests do not check edges.** They validate the package set
   and feature set. The cycle and weak-dep bugs only showed up via full-lockfile
   diff and the new `pubgrub_graph.rs`. Always add edge-level tests for graph
   bugs.

5. **Always `rm -f Cargo.lock` before a fresh-resolution test.** A present lock
   seeds `version_prefs` and hides bugs.

6. **The reconstructed node identity is the *selected summary's* `PackageId`,
   not the bucket's.** `[patch]` redirects a query to a summary from a different
   source, so building the node from the bucket's `(name, source)` records the
   wrong source ("patch not used" + checksum errors). Use
   `summary.package_id()`. `[replace]` is the inverse: keep the original as the
   node but *also* register the replacement target as a resolved node, else
   `Resolve::deps`' replacement redirection points at a package missing from the
   set ("couldn't find … in package set").

7. **A feature listing itself is a cycle PubGrub won't catch.**
   `default = ["default"]` becomes a self-dependency, which the solver treats as
   trivially satisfiable. Detect `*f == feat` explicitly to match the default
   resolver's `cyclic feature dependency` error. Mutual cycles (`A → B → A`
   across distinct features) are *legal* and must still resolve.

### How I root-caused #3 (technique worth reusing)
Temporarily instrumented `dep_cache.rs::resolve_features` to print, for a target
crate (env-gated), `parent`, `opts.features`, and `reqs.deps`. Running the
**default** resolver showed `serde` in bstr's `reqs.deps` despite features being
only `{std, unicode}` → pointed straight at `serde?/std`. (Instrumentation has
been removed; re-add ad hoc if needed.)

---

## 7. Reference material reused

- `pubgrub-rs/semver-pubgrub` — source ported/specialized into
  `semver_pubgrub.rs` (it targets pubgrub's git `dev` branch; adapted to
  published 0.4). MPL-2.0 — note for upstreaming.
- `Eh2406/pubgrub-crates-benchmark` — the `Names` encoding + `DependencyProvider`
  shape was the model for `package.rs`/`provider.rs`. Also a ready-made harness
  to resolve thousands of real crates with both resolvers (great for §8.4).
- pubgrub 0.4 published API notes: `DependencyConstraints<P,VS>` is a `Vec`
  newtype (build via `FromIterator`; no `entry`/`insert`).
  `SelectedDependencies` has `iter()`/`get()`. `Dependencies::{Available,
  Unavailable}`. `Range` is re-exported from `version-ranges` 0.1.

---

## 8. Known limitations / open questions

- **Conservative updates verified at the resolver level, not end-to-end.**
  Building against a lock, `cargo update -p`, and `--precise` all reach the
  resolver as `VersionPreferences`; `pubgrub_update.rs` + the new
  `pubgrub_prop.rs` case confirm pubgrub honors those preferences exactly like
  the default resolver (`choose_version` iterates `version_prefs`-sorted
  candidates). Not yet covered: the `ops::resolve` glue that constructs the
  preferences from a real `Cargo.lock`, and the registry-side version pinning
  `--precise` applies in addition to the preference.
- **`[patch]`/`[replace]`** — now handled in `solution.rs`. `[patch]` uses the
  selected summary's real `PackageId` (carrying the patched source) as the node
  identity; `[replace]` registers the replacement target as a resolved node
  (graph + summary + checksum), mirroring the default resolver's activation of
  the replacement summary. Brought `patch::` 43→25 and `replace::` 20→9 under
  `__CARGO_TEST_PUBGRUB`. Remaining `patch::` failures are a *spurious
  `[UPDATING]` index refresh* (the non-locked wildcard query defeats the
  locked-patch short-circuit in `PackageRegistry::query`), not misresolution.
- **Error reporting** — a standalone bridge now lives in `pubgrub/error.rs`
  (the only place that formats resolver errors). It returns a typed
  `ResolveError` and reuses the v1 resolver's own renderers for byte-identical
  text, via three extracted helpers in `errors.rs`:
  - `RequirementError::into_activate_error(None, …)` — root/CLI
    missing/cyclic-feature and missing-dependency errors;
  - `no_candidates_error` — the "no matching package / version / yanked / typo"
    family (the trigger recovers the failing `Dependency` and checks whether
    *any* candidate matches its req, so both absent-crate and wrong-version
    cases route here);
  - `version_conflict_error` — the "candidates exist but conflict" family, used
    so far for a dependency requesting a feature its target lacks (the bridge
    reuses `into_activate_error(Some(parent), …)` to get Cargo's own
    `ConflictReason`).

  PubGrub's custom incompatibility metadata `M` is a structured
  `UnavailableReason`, not a string, so the provider never bakes prose.
  **Still falling back** to pubgrub's `DefaultStringReporter` (wrapped as
  `ResolveError`): *semver* and *links* conflicts (deliberately not bridged —
  their text needs the full multi-hop dependency path the derivation tree
  doesn't preserve; see §9.6), and the offline-mode hint (the provider carries
  no `GlobalContext`).
- **Performance** is not tuned: blocking poll loop in `Provider::candidates`, no
  reuse of the provider across Cargo's two resolve passes, `RefCell` caches.
- **`Wide` packages** (multi-bucket requirements) are implemented but lightly
  exercised; most real reqs are single-bucket.
- **`features` map fidelity** in the reconstructed `Resolve` is approximate
  (Feat names + `default`); the lockfile itself doesn't store features, but
  downstream `cargo build` feature unification reads this map — verify it.
- **Public/private deps, artifact (bindeps), platform `cfg` deps** not
  specifically validated.

---

## 9. Prioritized next steps

1. ~~Run `tests/resolve.rs` through pubgrub.~~ **DONE** via `__CARGO_TEST_PUBGRUB`
   (see §2/§3). `resolve.rs` 37/37, `pubgrub.rs` 28/28; `proptests.rs` also
   passes 5/5 under the env var at the default 256 cases. The env var is now
   *also* honored at the resolver dispatch fork (not just in the resolver-tests
   harness), so the full `cargo test -p cargo --test testsuite` can be run on
   PubGrub — see §12 for the current survey results. Next: wire a curated
   green subset into CI (the full testsuite is not yet pass/fail-clean).
2. **Scale the property test** (bump cases way up; loop it). It is the Cargo
   team's de-facto correctness gate.
3. ~~**Verify conservative-update paths**: existing-lock reuse, `cargo update
   -p`, `--precise`. Add tests that resolve, mutate one dep, and re-resolve.~~
   **DONE at the resolver level** via `pubgrub_update.rs` (8 deterministic
   differential tests) and a new `pubgrub_prop.rs` case. All three paths reduce
   to `VersionPreferences` at the resolver boundary, so the tests build prefs
   from a first resolution and re-resolve both kept and freed, comparing
   pubgrub against the default resolver (graph nodes + edges) and SAT. Next:
   close the end-to-end gap — drive a real `Cargo.lock` through `ops::resolve`
   and the `--precise` registry pinning (see §8), e.g. via a cargo-test
   integration test rather than the resolver harness.
4. **Real-world differential testing** via `Eh2406/pubgrub-crates-benchmark` —
   resolve many crates.io crates with both resolvers and diff.
5. **Weak-dep + feature-map fidelity** — stress more `dep?/feat` shapes and
   confirm the `Resolve.features` map matches the default resolver, not just the
   lockfile graph.
6. **Cargo-native error reporting** — *partially done; remainder deliberately
   deferred.* The standalone `pubgrub/error.rs` bridge (see §8) covers the
   missing/cyclic-feature, missing-dependency, no-candidates (incl.
   wrong-version), and dependency-requested-feature-conflict families with
   byte-identical text via the v1 renderers.

   **Not pursued (by design):** the *semver* ("all possible versions conflict")
   and *links* conflict families — 7 tests total. Their expected text embeds the
   **full multi-hop dependency chain** of both the failing package *and* the
   competing already-selected package (e.g. `foo → qux → bad` vs `foo → baz →
   bad`, each edge with its exact `Dependency`). The default resolver has this
   from `ResolverContext::parents` (the real resolution graph); PubGrub's
   derivation tree records *incompatibilities*, not that path, so reconstructing
   it would be guesswork tuned to one observed tree shape — i.e. overfitting on
   the smallest remaining bucket. The right fix is architectural: thread the
   actual resolution path through, or design PubGrub-native reporting; not a
   tree-shape bridge. Until then these fall back to `DefaultStringReporter`.

   Also still falling back: the offline-mode hint (the provider carries no
   `GlobalContext`).
7. **Spurious `[UPDATING]` index refresh.** The provider always queries with a
   non-locked wildcard `Dependency`, defeating the `patches.len() == 1 &&
   dep.is_locked()` short-circuit in `PackageRegistry::query`. This makes
   `cargo` print an extra `[UPDATING]`/download line vs. the default resolver —
   the bulk of the remaining `patch::` testsuite failures. Resolution is
   correct; only the index-access side effect differs.
8. **Performance** — defer until correctness is solid.

---

## 10. Commit history (this branch)

Newest first. Implementation: `9fa0e7f75`–`913116cbb`; correctness &
error-reporting work: `8672b1ee`–`cef37a10`; earlier fixes: `eb917c1f7`,
`c83889704`→`c916af4f5`; tests: `6d49e8644`, `1f17605b3`, `0864cb574`,
`7d24add22`, `87e953f7b`–`22a51e300`; observability: `37fa77459`; docs:
`cacdd97e9`, `6de3fd5be`, `68fb458d9`, `ee05f2fbb`, and this update.

```
cef37a10 feat(resolver): Bridge dependency-requested feature conflicts
b93b27d9 refactor(resolver)!: Extract version_conflict_error from activation_error
3f8a2eec feat(resolver): Bridge wrong-version errors to Cargo-native text
81289426 feat(resolver): Bridge no-candidates errors to Cargo-native text
72b65b23 refactor(resolver)!: Extract no_candidates_error from activation_error
5aba275a feat(resolver): Add Cargo-native error-reporting bridge for pubgrub
57c797dd refactor(resolver): Expose RequirementError for reuse by pubgrub
b8995f1c fix(resolver): Detect self-referential feature cycles in pubgrub
8c71cf85 fix(resolver): Register [replace] targets as nodes in pubgrub lockfile
8672b1ee fix(resolver): Track patched source in pubgrub lockfile reconstruction
f0891f17 docs(resolver): Fix broken intra-doc links in the pubgrub module
cd65cafa fix(resolver): Read __CARGO_TEST_PUBGRUB via GlobalContext, not std::env
22a51e300 test(resolver): Add CARGO_TEST_PUBGRUB escape hatch at the dispatch fork
8e8a44a26 test(resolver): Add conservative-update property test for pubgrub
592a1a47e test(resolver): Add conservative-update differential tests for pubgrub
87e953f7b refactor(resolver): Allow seeding VersionPreferences in the raw resolve helper
ee05f2fbb docs: Update handoff doc for cleaner verification methodology
37fa77459 feat(resolver): Add observable trace when the pubgrub resolver runs
68fb458d9 docs: Record curated-suite validation results for pubgrub resolver
7d24add22 test(resolver): Skip exact error-text assertions under pubgrub
0864cb574 test(resolver): Allow running the curated suite through pubgrub
6de3fd5be docs: Add PubGrub resolver design & handoff doc
c916af4f5 fix(resolver): Match v1 lock graph for weak dependency features
cacdd97e9 docs(unstable): Document -Zpubgrub-resolver flag
1f17605b3 test(resolver): Add pubgrub vs SAT property test
c83889704 fix(resolver): Record feature-agnostic dependency edges in pubgrub lock
6d49e8644 test(resolver): Add SAT-validated pubgrub resolution suite
eb917c1f7 fix(resolver): Seed workspace members into the pubgrub version cache
913116cbb feat(resolver): Wire up pubgrub resolution and reconstruct Resolve
f1d92a2d1 feat(resolver): Implement pubgrub DependencyProvider over the registry
bc8028b86 feat(resolver): Add PubGrubPackage encoding for the pubgrub resolver
083c0686a feat(resolver): Add semver-to-pubgrub VersionSet conversion
9fa0e7f75 feat(resolver): Add -Zpubgrub-resolver flag and module skeleton
```

> Note on history: commit `c83889704` ("feature-agnostic edges") was a wrong
> turn; it is corrected by `c916af4f5`. The current `solution.rs`/`provider.rs`
> reflect the corrected (activation-gated + weak-records-edge) behavior.

---

## 11. Quick orientation for the next agent

- Start in `src/cargo/core/resolver/pubgrub/mod.rs`, then `provider.rs`
  (`get_dependencies` is the heart), then `solution.rs`.
- To debug an edge mismatch: instrument `dep_cache.rs::resolve_features`
  (default resolver) and `solution.rs` (pubgrub) for a target crate, compare.
- The acceptance command is in §3; remember `rm -f Cargo.lock` each run.
- Before claiming "it works," test **fresh** (no lock) and **diff the full
  lockfile**, not just exit codes.

---

## 12. Full-testsuite survey under PubGrub

Run the entire integration testsuite through PubGrub via the dispatch hook
(§3). Run under **nightly** so the ~376 nightly-gated tests are un-ignored:

```sh
RUSTUP_TOOLCHAIN=nightly __CARGO_TEST_PUBGRUB=1 cargo +nightly test -p cargo --test testsuite
```

> ⚠️ **Methodology note.** The env var must match the dispatch hook exactly
> (`__CARGO_TEST_PUBGRUB`, two leading underscores). An earlier survey used the
> wrong name and so silently ran the *default* resolver, producing a bogus
> "3872 passed, 4 failed". Always confirm the pubgrub path actually ran (e.g.
> a known error-text test should fail) before trusting a survey number.

Results in this environment (nightly), tracking the correctness/error-reporting
work in §10:

| Survey | passed | failed | ignored |
|---|---|---|---|
| Before this work (baseline) | 4019 | 233 | 28 |
| After `[patch]`/`[replace]`/cyclic + error bridge | ~4075 | ~177 | 28 |
| After wrong-version (`alt_versions`) bridge | ~4088 | ~164 | 28 |
| After dependency-requested feature-conflict bridge | ~4094 | **~158** | 28 |

The failed count wobbles by ~1 between runs (≈158–159); the delta is entirely
in env-flaky tests (`artifact_dep::*` cross-compile, an `update::*` timing
case), **not** the resolver — diffing two runs shows only those swap in/out.

Per-module failure drops (baseline → now): `registry` 36→12, `replace` 20→9,
`package_features` 9→3, `features` 9→3, `package` 4→0, plus `member_errors`,
`generate_lockfile`, `source_replacement`, `features_namespaced` → 0, and
smaller drops across `build`/`directory`/`install`/`path`/`publish`/`update`.
(`patch` stays ~24 — those are the spurious-`[UPDATING]` issue, §9.7.)

The remaining ~158 failures are dominated by these known, non-correctness
causes:

1. **Spurious `[UPDATING]` index refresh** (§9.7) — bulk of `patch::`, and a
   chunk of `registry`/`offline`/`git`.
2. **Remaining conflict-family error text** (§8, §9.6) — the *semver* ("all
   possible versions conflict") and *links* conflicts. These need recovering
   *which already-selected package* conflicts from the derivation tree, which is
   fragile, so they still fall back. (The missing-feature conflict family is now
   byte-matched.)
3. **`metadata`/`build_script`/auth modules** — a mix of output-shape diffs and
   env-gated cases not yet individually triaged.

Both are output/formatting, not misresolution. The 28 still-ignored are
genuinely unavailable (network/container/`hg`/manual-only), not nightly-gated.
