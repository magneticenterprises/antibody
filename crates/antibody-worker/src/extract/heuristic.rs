//! Rule-based extraction that works with no model provider at all.

use super::{Candidate, Extraction, HEURISTIC_EXTRACTOR, applicability_for, sanitize};
use antibody_core::api::{SessionIngestRequest, TranscriptMessage};
use antibody_core::model::MessageRole;
use antibody_core::normalize;
use antibody_core::text::one_line;
use regex::Regex;
use std::sync::LazyLock;

static RE_ROOT_CAUSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:root cause|caused by|the (?:problem|issue|bug) (?:was|is)|this happens because|because)\b(?:[^.\n]|\.\S){10,300}[.\n]?").unwrap()
});
static RE_TESTS_PASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:all )?(?:tests?|specs?|checks?) (?:now )?(?:pass|passed|passing|green)\b|\btest result: ok\b").unwrap()
});

fn last_text(msgs: &[TranscriptMessage], role: MessageRole) -> Option<&str> {
    msgs.iter()
        .rev()
        .filter(|m| m.role == role && m.tool_name.is_none())
        .map(|m| m.content.as_str())
        .find(|c| c.trim().len() > 20)
}

fn first_text(msgs: &[TranscriptMessage], role: MessageRole) -> Option<&str> {
    msgs.iter()
        .filter(|m| m.role == role && m.tool_name.is_none())
        .map(|m| m.content.as_str())
        .find(|c| !c.trim().is_empty())
}

/// Rule-based extraction that works with no model provider at all.
pub fn heuristic(req: &SessionIngestRequest) -> Extraction {
    let msgs = &req.messages;
    let hints = req.hints.clone().unwrap_or_default();

    let error_corpus: String = msgs
        .iter()
        .filter(|m| matches!(m.role, MessageRole::Tool | MessageRole::User))
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut signatures: Vec<String> = hints
        .error_signatures
        .iter()
        .map(|s| normalize::normalize_error_signature(s))
        .filter(|s| s.len() >= 6)
        .collect();
    for s in normalize::extract_error_signatures(&error_corpus, 10) {
        if !signatures.contains(&s) && signatures.len() < 10 {
            signatures.push(s);
        }
    }

    let final_assistant = last_text(msgs, MessageRole::Assistant).unwrap_or("");
    let remedy_text = hints
        .remedy
        .clone()
        .unwrap_or_else(|| final_assistant.to_string());
    if signatures.is_empty() && hints.remedy.is_none() && hints.symptom.is_none() {
        // Nothing indicates a problem that was solved: do not invent knowledge.
        return Extraction {
            extractor: HEURISTIC_EXTRACTOR.into(),
            ..Default::default()
        };
    }
    if remedy_text.trim().len() < 15 {
        return Extraction {
            extractor: HEURISTIC_EXTRACTOR.into(),
            ..Default::default()
        };
    }

    let first_user = first_text(msgs, MessageRole::User).unwrap_or("");
    let symptom = hints.symptom.clone().unwrap_or_else(|| {
        let mut s = one_line(first_user, 400);
        if let Some(sig) = signatures.first()
            && !s
                .to_lowercase()
                .contains(&sig.chars().take(30).collect::<String>())
        {
            s = format!("{s} — {sig}");
        }
        s
    });
    let title = hints
        .title
        .clone()
        .or_else(|| signatures.first().cloned())
        .unwrap_or_else(|| one_line(first_user, 100));

    let assistant_corpus: String = msgs
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let root_cause = hints.root_cause.clone().or_else(|| {
        RE_ROOT_CAUSE
            .find(&assistant_corpus)
            .map(|m| one_line(m.as_str(), 400))
    });

    let mut paths: Vec<String> = hints
        .files
        .iter()
        .map(|f| normalize::normalize_path(f))
        .collect();
    for p in normalize::extract_paths(&assistant_corpus, 20) {
        if !paths.contains(&p) && paths.len() < 20 {
            paths.push(p);
        }
    }
    let symbols =
        normalize::extract_symbols(&format!("{}\n{}", signatures.join("\n"), remedy_text), 20);
    let corpus = format!("{error_corpus}\n{assistant_corpus}");
    let claims_tests_passed =
        !hints.tests_run.is_empty() || RE_TESTS_PASS.is_match(final_assistant);

    let mut confidence: f32 = 0.4;
    if req.hints.is_some() {
        confidence += 0.1;
    }
    if !signatures.is_empty() {
        confidence += 0.1;
    }
    if claims_tests_passed {
        confidence += 0.05;
    }

    Extraction {
        extractor: HEURISTIC_EXTRACTOR.into(),
        candidates: vec![sanitize(Candidate {
            title,
            symptom,
            root_cause,
            remedy: remedy_text,
            error_signatures: signatures,
            symbols,
            paths,
            applicability: applicability_for(req, &corpus),
            confidence: confidence.min(0.7),
            claims_tests_passed,
        })],
        workflow: vec![],
    }
}
