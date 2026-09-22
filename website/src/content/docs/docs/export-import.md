---
title: Portability (export/import)
description: Own your memory — `.mem` archives now carry the entity graph, merge with dedupe semantics, and round-trip cleanly.
---

"Own your memory system" starts with *can you take it elsewhere?* statefulmemory
archives are portable `.mem` files: single-file, checksummed, optionally
seed-encrypted, and since v2 they carry the **entity graph** along with
observations, facts, relations, sessions, and prompts.

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/decide-mem-v2.mp4" title="Decide and mem archives">
  Your browser does not support video.
</video>

## Format

- `SMEM` magic + MessagePack + **zstd level 19**.
- Default XOR obfuscation; optional **Argon2id + XChaCha20-Poly1305** seed
  encryption (`--seed-file` / `--seed-phrase`).
- Deterministic ordering — an archive diff is readable in `git diff`.

## Export / import

```bash
statefulmemory mem export --out backup.mem
statefulmemory mem export --out backup.mem --seed-phrase "…"     # encrypted

statefulmemory mem import backup.mem                              # merge (default)
statefulmemory mem import backup.mem --mode replace               # wipe + insert
```

`.mem` v2 includes `entities`, `entity_mentions`, and `entity_edges`; imports
remap entity ids by `norm_name` (upsert) so a merge **dedupes** cleanly, and
mentions/edges follow entity + observation maps. Re-importing the same archive
is idempotent. Backward compatible: v1 archives (no graph payload) still
decode — the graph fields are `serde`-optional.

## No lock-in

- `statefulmemory mem export` is your **data-removal / portability story** for
  compliance questionnaires.
- `graph rebuild` re-derives entities/edges from anchors after an import if a
  graph payload is absent.
- Archives are inspectable offline: they are just a structured dump of the
  SQLite project DB.

## CI guard

`.github/workflows/mem-roundtrip.yml` runs export → wipe → import on every PR
and asserts the imported graph is non-empty — the round-trip guarantee is a
regression gate, not a hope.