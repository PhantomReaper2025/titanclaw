use super::paths;

pub const CORE_DOC_TEMPLATE_VERSION: u32 = 2;
pub const CORE_DOC_MARKER_PREFIX: &str = "<!-- titanclaw:core-doc";

pub struct CoreDocTemplate {
    pub path: &'static str,
    pub body: &'static str,
    pub legacy_exact: &'static [&'static str],
}

pub const HEARTBEAT_SEED_BODY: &str = "\
# Heartbeat Checklist

<!-- Keep this file empty to skip heartbeat API calls.
     Add tasks below when you want the agent to check something periodically.

     Example:
     - [ ] Check for unread emails needing a reply
     - [ ] Review today's calendar for upcoming meetings
     - [ ] Check CI build status for main branch
-->";

const README_LEGACY: &str = "\
# Workspace

This is your agent's persistent memory. Files here are indexed for search
and used to build the agent's context.

## Structure

- `MEMORY.md` - Long-term notes and facts worth remembering
- `IDENTITY.md` - Agent name, nature, personality
- `SOUL.md` - Core values and principles
- `AGENTS.md` - Behavior instructions for the agent
- `USER.md` - Information about you (the user)
- `HEARTBEAT.md` - Periodic background task checklist
- `daily/` - Automatic daily session logs
- `context/` - Additional context documents

Edit these files to shape how your agent thinks and acts.";

const MEMORY_LEGACY: &str = "\
# Memory

Long-term notes, decisions, and facts worth remembering.
The agent appends here during conversations.";

const IDENTITY_LEGACY: &str = "\
# Identity

Name: IronClaw
Nature: A secure personal AI assistant

Edit this file to give your agent a custom name and personality.";

const SOUL_LEGACY: &str = "\
# Core Values

- Protect user privacy and data security above all else
- Be honest about limitations and uncertainty
- Prefer action over lengthy deliberation
- Ask for clarification rather than guessing on important decisions
- Learn from mistakes and remember lessons";

const AGENTS_LEGACY: &str = "\
# Agent Instructions

You are a personal AI assistant with access to tools and persistent memory.

## Guidelines

- Always search memory before answering questions about prior conversations
- Write important facts and decisions to memory for future reference
- Use the daily log for session-level notes
- Be concise but thorough";

const USER_LEGACY: &str = "\
# User Context

The agent will fill this in as it learns about you.
You can also edit this directly to provide context upfront.";

const README_BODY: &str = "\
# Workspace Operator Guide

This workspace is TitanClaw's long-term operating context. Files here are persisted, indexed, and re-used across sessions.

## Core Files

- `AGENTS.md`: operational behavior and constraints for the assistant
- `IDENTITY.md`: identity, tone, and role boundaries
- `SOUL.md`: non-negotiable values and principles
- `USER.md`: facts and preferences about the human operator
- `MEMORY.md`: durable decisions, commitments, and critical facts
- `HEARTBEAT.md`: periodic checklist for background scanning

## Working Rules

1. Write durable facts to `MEMORY.md`.
2. Keep identity and behavior docs current when goals change.
3. Use `daily/YYYY-MM-DD.md` for session logs and temporary notes.
4. Keep sensitive secrets in secure settings, not plain workspace files.

## Suggested Maintenance Cadence

- Daily: prune stale notes from daily logs.
- Weekly: promote important items into `MEMORY.md`.
- Before major upgrades: update `AGENTS.md` and `IDENTITY.md` first.";

const MEMORY_BODY: &str = "\
# Long-Term Memory

Use this file for facts worth retaining beyond a single chat.

## Capture Format

- Date:
- Topic:
- Decision / Fact:
- Why it matters:
- Follow-up:

## Important

- Prefer concise, verifiable statements.
- Remove obsolete items instead of accumulating contradictions.
- If a fact affects behavior, mirror it in `AGENTS.md` or `USER.md`.";

const IDENTITY_BODY: &str = "\
# Identity

Name: TitanClaw
Role: Secure automation and coding copilot
Mode: Direct, reliable, execution-focused

## Boundaries

- Do not fabricate results.
- Flag uncertainty with clear next validation steps.
- Prioritize safety, correctness, and operator intent over speed.

## Style

- Communicate in short, concrete statements.
- Surface blockers early with actionable remediation.
- Prefer implementation and verification over abstract planning.";

const SOUL_BODY: &str = "\
# Core Values

1. Protect user data and credentials.
2. Prefer truthful, testable outcomes over persuasive wording.
3. Keep changes reversible and well-documented.
4. Treat reliability regressions as high priority.
5. Preserve user agency: ask before destructive actions.";

const AGENTS_BODY: &str = "\
# Agent Operating Instructions

## Execution Contract

- Convert user intent into completed, verified changes.
- Run targeted checks after edits and report failures precisely.
- Keep plans synchronized with implementation artifacts and docs.

## Memory Discipline

- Read relevant workspace context before high-impact changes.
- Persist decisions and invariants into `MEMORY.md`.
- Keep identity files aligned with actual behavior.

## Quality Bar

- Avoid partial implementations when completion is feasible.
- Do not silently skip failing checks.
- Call out residual risk explicitly.";

const USER_BODY: &str = "\
# User Profile

Track stable operator preferences here.

## Preferences

- Communication style:
- Risk tolerance:
- Stack / tools:
- Product priorities:

## Notes

Keep this file factual and current. Remove stale assumptions quickly.";

const HEARTBEAT_BODY: &str = HEARTBEAT_SEED_BODY;

pub fn core_doc_templates() -> &'static [CoreDocTemplate] {
    static TEMPLATES: &[CoreDocTemplate] = &[
        CoreDocTemplate {
            path: paths::README,
            body: README_BODY,
            legacy_exact: &[README_LEGACY],
        },
        CoreDocTemplate {
            path: paths::MEMORY,
            body: MEMORY_BODY,
            legacy_exact: &[MEMORY_LEGACY],
        },
        CoreDocTemplate {
            path: paths::IDENTITY,
            body: IDENTITY_BODY,
            legacy_exact: &[IDENTITY_LEGACY],
        },
        CoreDocTemplate {
            path: paths::SOUL,
            body: SOUL_BODY,
            legacy_exact: &[SOUL_LEGACY],
        },
        CoreDocTemplate {
            path: paths::AGENTS,
            body: AGENTS_BODY,
            legacy_exact: &[AGENTS_LEGACY],
        },
        CoreDocTemplate {
            path: paths::USER,
            body: USER_BODY,
            legacy_exact: &[USER_LEGACY],
        },
        CoreDocTemplate {
            path: paths::HEARTBEAT,
            body: HEARTBEAT_BODY,
            legacy_exact: &[HEARTBEAT_SEED_BODY],
        },
    ];
    TEMPLATES
}

pub fn render_template_content(template: &CoreDocTemplate) -> String {
    format!(
        "{} version={} path={} -->\n\n{}",
        CORE_DOC_MARKER_PREFIX, CORE_DOC_TEMPLATE_VERSION, template.path, template.body
    )
}
