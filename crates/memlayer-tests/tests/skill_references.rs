//! Integration tests for SKILL.md progressive-disclosure (aid-t4, aid-t5).
//!
//! Spec coverage: SC-4 (core ≤60 lines + ≥5 references/ links), SC-5
//! (memlayer install copies all 5 sidecars under references/ atomically).

use memlayer_tests::CliEnv;

const EXPECTED_REFS: &[&str] = &[
    "types.md",
    "slash.md",
    "hooks.md",
    "failures.md",
    "examples.md",
];

// ---------------------------------------------------------------------------
// SC-5: `memlayer install` writes all 5 sidecars under references/
// ---------------------------------------------------------------------------

#[test]
fn install_writes_all_five_sidecars() {
    let env = CliEnv::new();

    // Target Claude Code explicitly so sidecars are always written under the
    // test HOME, even when no agent is auto-detected on the runner PATH.
    let out = env
        .cmd()
        .args(["install", "--agent", "claude-code"])
        .output()
        .expect("run install");
    assert!(
        out.status.success(),
        "memlayer install failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    // SC-5: every sidecar exists under <HOME>/.claude/skills/memlayer/references/.
    let refs_dir = env
        .data_path()
        .join(".claude")
        .join("skills")
        .join("memlayer")
        .join("references");

    for name in EXPECTED_REFS {
        let path = refs_dir.join(name);
        assert!(
            path.exists(),
            "expected sidecar at {} after install",
            path.display(),
        );
    }
}

#[test]
fn sidecar_bytes_match_source() {
    let env = CliEnv::new();
    let out = env
        .cmd()
        .args(["install", "--agent", "claude-code"])
        .output()
        .expect("install must succeed");
    assert!(
        out.status.success(),
        "memlayer install failed: stderr={}",
        String::from_utf8_lossy(&out.stderr),
    );

    let installed_dir = env
        .data_path()
        .join(".claude")
        .join("skills")
        .join("memlayer")
        .join("references");

    // Source-of-truth bodies bundled into the binary at compile time.
    let source = [
        ("types.md", include_str!("../../../skills/memlayer/references/types.md")),
        ("slash.md", include_str!("../../../skills/memlayer/references/slash.md")),
        ("hooks.md", include_str!("../../../skills/memlayer/references/hooks.md")),
        ("failures.md", include_str!("../../../skills/memlayer/references/failures.md")),
        ("examples.md", include_str!("../../../skills/memlayer/references/examples.md")),
    ];

    for (name, expected) in source {
        let installed = std::fs::read_to_string(installed_dir.join(name))
            .unwrap_or_else(|e| panic!("read {name}: {e}"));
        assert_eq!(
            installed, expected,
            "installed {name} bytes diverged from compile-time source",
        );
    }
}

// ---------------------------------------------------------------------------
// SC-4: SKILL.md core stays small with sidecar links.
// Verified once at compile time via the include_str! body length below;
// also checked here with simple counts.
// ---------------------------------------------------------------------------

#[test]
fn skill_core_under_60_lines_and_links_to_5_sidecars() {
    let core: &str = include_str!("../../../skills/memlayer/SKILL.md");
    let line_count = core.lines().count();
    assert!(
        line_count <= 60,
        "SKILL.md core must be ≤60 lines, got {line_count}",
    );
    let ref_link_count = core.matches("references/").count();
    assert!(
        ref_link_count >= 5,
        "SKILL.md must link to ≥5 sidecars, got {ref_link_count} 'references/' occurrences",
    );
}
