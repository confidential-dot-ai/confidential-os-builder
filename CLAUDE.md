# Lunal Engineering Standards

These standards apply to all code written in Lunal repositories. Claude must follow these when generating, reviewing, or modifying code.

## How to Write Code

**Before writing code:**

1. **Search for existing solutions.** Check the codebase, standard library, project dependencies, then **search the internet** for well-maintained libraries. Don't hand-roll what someone has already solved. If you're about to write a parser, converter, protocol handler, retry loop, or anything non-trivial — there's probably a crate/module for it. If you must wrap an existing tool, the wrapper should be thin.
2. **Start with the simplest approach.** Shell one-liner → config change → existing library → thin wrapper → new code. Only escalate when the simpler approach fails a specific, stated requirement. At each level, state what requirement forced the escalation.
3. **Classify the code.** Speed, correctness, or simplicity. This determines everything downstream.
4. **State the invariants.** What must always be true for this code to be correct? These are your acceptance criteria.
5. For speed-class code: write the benchmark before the implementation, with realistic workloads.

**While writing code:**

- **Correctness-class** (crypto, attestation, auth, consensus): Write the simplest, most auditable version. Fewer lines and branches. No early exits that skip validation. No caching of security decisions. No speed optimizations. Use distinct types to make misuse a compile error (e.g., separate types for plaintext/ciphertext, signed/unsigned, attested/unattested).
- **Speed-class** (hot paths, data processing): Measure first. Don't optimize without a benchmark. Know your allocation count per call and time complexity per function.
- **Simplicity-class** (glue, scripts, config): Could this be a shell one-liner or a config change? If yes, do that. No dashboards, UIs, or visualization layers unless explicitly requested.
- **All code:** Volume is not value. If a function exceeds 50 lines, justify why it can't be decomposed. Line count should be proportional to problem complexity, not prompt complexity. Use `debug_assert!` (Rust) or build-tag-gated checks (Go) to verify critical invariants at runtime during development.

**After writing code, find what's wrong:**

1. Identify the 3 most likely semantic bugs (wrong algorithm, wrong trust boundary, missing edge case — not syntax).
2. What would an attacker try?
3. What existing tool or library could replace this code entirely?
4. Could this be simpler? Fewer lines? Fewer branches?

Do NOT say the code "looks good." Find what's most likely broken.

## Imports

All imports at the top of the file. No inline imports, no imports inside function bodies. All languages — Rust `use`, Go `import`, Python `import`, TypeScript `import`/`require`.

## Code Comments

Use when they add value — not on every function.

```
// INVARIANT: [specific invariant this code must satisfy]
// SAFETY: [why invariants hold]  (required for every unsafe block in Rust)
// DEVIATION: [why this deviates from the design doc]
```

## Rust

- No `.unwrap()` in library code. Use typed errors.
- Every `unsafe` block requires a `// SAFETY:` comment. Encapsulate behind safe APIs.
- `#[serde(deny_unknown_fields)]` on security-sensitive types.
- No locks held across `.await` points.
- Integer casts from untrusted input use `try_from`, not `as`.

## Go

- Every goroutine must have a shutdown path (typically via `context.Context`).
- Error wrapping preserves chain: `fmt.Errorf("operation: %w", err)`.
- `go test -race` and `govulncheck` mandatory in CI.
- `io.LimitReader` for untrusted input streams.

## Distributed Systems

- Name the consistency model (linearizable, sequential, eventual, causal). Justify the choice.
- Every RPC must have a timeout. Every timeout must be configurable.
- State what happens during a network partition for every operation.
- Every write operation must state whether it's idempotent and how duplicate detection works.
- Clock assumptions must be explicit: does this code require synchronized clocks? Within what bound?

## Cryptography

- Never implement a cryptographic primitive. Use audited libraries (`ring`, `boring`, `crypto/subtle`).
- Constant-time comparisons for secrets. No `==` on secrets. No branching on secret data.
- No secret-dependent memory access patterns.
- Key material zeroized on drop (`zeroize` crate in Rust, `explicit_bzero` in C/Go).
- Nonce/IV uniqueness enforced, especially in distributed systems. Nonce reuse in AES-GCM is catastrophic.
- Approved KDFs only: HKDF for key derivation, Argon2id for passwords.
- RNG inside TEEs must come from hardware sources, not the untrusted host.
- Use distinct types for plaintext vs. ciphertext, signed vs. unsigned, attested vs. unattested. Make confusion a compile error.
- State the threat model: who is the attacker, what do they control, what are they trying to learn.

## TEE / Attestation

- Verify attestation report signature before reading any field. The report is untrusted until verified.
- The host is adversarial. Validate all data entering the TEE.
- No trusting host-provided time or randomness inside the TEE.
- Minimize code inside the TEE boundary.
- See `reference/tee-security.md` for platform-specific details.

## Secrets and Logging

- Never log: cryptographic keys, attestation secrets, plaintext of encrypted data, PII, auth tokens.
- Always log: auth decisions, attestation results (pass/fail, not raw reports), config changes, key lifecycle events.
- `Debug`/`Display` impls on secret-holding types must redact values.
- Secrets injected at runtime via vault/KMS, never in source or images.
- No secrets in source code.

## Dependencies

- No new dependency without justification.
- Pin versions. Container images by SHA256 digest.
- `cargo-audit` (Rust) and `govulncheck` (Go) in CI.

## Anti-Patterns

Reject these immediately:
1. Sophisticated solution to a simple problem. What's the shell one-liner?
2. Right names, wrong behavior. Read the implementation, not just the function name.
3. Tests that verify output format but not the invariant. Ask: what bug would this test NOT catch?
4. LLM self-review that praises the code. Find the three most likely bugs instead.
5. Reimplementing without searching for existing solutions first.
6. Speed-optimizing correctness-class code. Every early exit is a potential bypass. Every cache is a potential TOCTOU.
7. Lines of code or "estimated dev cost" presented as evidence of value.
8. "Safe defaults" stacked without analysis. `sync_all` + `Mutex` + `.clone()` + per-request allocation — each individually defensible, collectively catastrophic on a hot path.
