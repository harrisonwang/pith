//! Doc-sync tests: guidance surfaces (SKILL.md / docs/API_REFERENCE.md) must track the real
//! error contract. Stale guidance is worse than none — an agent that loads
//! drifted instructions matches strings that no longer exist and misroutes
//! every failure. Same discipline as `table_usage_lists_every_narrowing_flag`
//! in src/cli.rs: derive from the code, assert the docs.

use std::path::Path;

fn read_doc(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {relative}: {e}"))
}

/// Every stable error code must be documented where agents load guidance
/// from (SKILL.md) and where wrapper authors copy from (docs/API_REFERENCE.md). Only the
/// snake_case codes are pinned — the Chinese reason/hint texts are display
/// copy and may be reworded freely.
#[test]
fn skill_and_api_reference_document_every_error_code() {
    let skill = read_doc("skills/spoor/SKILL.md");
    let api = read_doc("docs/API_REFERENCE.md");

    for code in spoor_core::ErrorCode::ALL {
        let code = code.as_str();
        assert!(
            skill.contains(code),
            "skills/spoor/SKILL.md 缺少错误码 {code} 的处置指引"
        );
        assert!(api.contains(code), "docs/API_REFERENCE.md 的错误码表缺少 {code}");
    }
}

/// Successful parsing can still be incomplete. Every stable warning code must
/// be taught to agents and listed for wrapper authors.
#[test]
fn skill_and_api_reference_document_every_warning_code() {
    let skill = read_doc("skills/spoor/SKILL.md");
    let api = read_doc("docs/API_REFERENCE.md");

    for code in spoor_core::WarningCode::ALL {
        let code = code.as_str();
        assert!(
            skill.contains(code),
            "skills/spoor/SKILL.md 缺少完整性警告码 {code} 的处置指引"
        );
        assert!(
            api.contains(code),
            "docs/API_REFERENCE.md 的 warning code 表缺少 {code}"
        );
    }
}

/// SKILL.md must teach the two stable truncation signals — the `> [!WARNING]`
/// block prefix and the JSON `truncated` field — and must not teach matching
/// the old English marker text that no longer exists.
#[test]
fn skill_teaches_stable_truncation_signals() {
    let skill = read_doc("skills/spoor/SKILL.md");

    assert!(skill.contains("> [!WARNING]"));
    assert!(skill.contains("truncated"));
    assert!(
        !skill.contains("Content is incomplete"),
        "SKILL.md 不应再教匹配已被汉化移除的英文截断文案"
    );
    assert!(
        !skill.contains("image-only PDF"),
        "SKILL.md 不应再教按旧英文 reason 文本分支；应按 code 分支"
    );
}

#[test]
fn skill_teaches_docx_image_placeholder_protocol() {
    let skill = read_doc("skills/spoor/SKILL.md");

    // SKILL must use the unified `spoor://<fmt>/<model>/...` shape, not the
    // retired per-format scheme, so a fresh agent learns the current protocol.
    assert!(skill.contains("spoor://docx/part/word/media/"));
    assert!(
        skill.contains("spoor document.docx --extract spoor://docx/part/word/media/image1.png")
    );
    assert!(!skill.contains("spoor-docx://"));
    assert!(!skill.contains("unzip -p"));
}
