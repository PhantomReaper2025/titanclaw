# Heartbeat Checklist

Use this file for recurring checks TitanClaw should run in background routines.

## Active Checks

- [ ] Verify OpenRouter/API provider health and auth status
- [ ] Check failed sandbox jobs in the last 24h and summarize root causes
- [ ] Report pending implementation items from `implementation_plan.md`
- [ ] Alert on repeated channel/web gateway disconnect patterns

## Execution Notes

- Keep checks specific and observable.
- Remove stale checks instead of accumulating dead tasks.
- If a check needs secrets or credentials, store those in secure settings, not here.
