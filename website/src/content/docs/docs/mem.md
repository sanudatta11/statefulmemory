---
title: Mem archives
description: Export and import portable .mem backups of project memory, optionally passphrase-protected.
---

A **`.mem` archive** is a portable snapshot of project memory you can copy,
backup, or move between machines. Export writes the archive; import restores
observations into the local daemon store.

<video class="ml-demo-video" controls playsinline muted preload="metadata" src="/videos/decide-mem.mp4" title="Decide and mem archives demo">
  Your browser does not support video.
</video>

## When to use it

- Back up before wiping `~/.memlayer/`
- Share a project memory pack with a teammate (mind secrets in notes)
- Encrypt an archive with a seed file / phrase for transit

## Export

```bash
memlayer mem export --out backup.mem
memlayer mem export --out secret.mem --seed-file ./phrase.txt
memlayer mem export --out secret.mem --seed-phrase 'your phrase here'
```

## Import

```bash
memlayer mem import backup.mem
memlayer mem import secret.mem --seed-file ./phrase.txt
```

Use the same seed that was used at export for encrypted archives.

## How it fits

Archives cover stored observations for the current project resolution — not
your agent MCP configs. Day-to-day editing still uses
[observations](/docs/observations/) and [verify](/docs/anchors-verify/).

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| Import decrypt error | Wrong `--seed-file` / phrase |
| Empty archive | Confirm project (`MEMLAYER_PROJECT` / git root) before export |
| Huge file | Normal for long histories; prune with soft-delete before export if needed |
