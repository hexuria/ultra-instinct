---
name: ultra-instinct
description: Drive a live Chrome browser with a plain-language goal using the ultra-instinct CLI. Use when the user asks to click, type, fill, navigate, or automate anything in a browser — e.g. "/ultra-instinct book a flight" or "submit this form for me".
---

# ultra-instinct

The full guide ships inside the `aui` binary, so it always matches the
installed version. Read it before you act:

```bash
command -v aui || bash scripts/install-aui.sh   # in a checkout of hexuria/ultra-instinct
command -v aui || curl -fsSL https://raw.githubusercontent.com/hexuria/ultra-instinct/main/scripts/install-aui.sh | bash
aui skills get core
```

Follow what it says: `aui run --goal "<the user's words, verbatim>" --cdp`
for a whole goal, or the shell verbs (`aui open`, `aui snap`, `aui click
@e5`, `aui wait`, `aui close`) to go step by step. `aui skills list` shows
every guide.
