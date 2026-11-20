# Anti-drift 7b: Harder page types

Rule 7b of the anti-drift block indexed in [AGENTS.md](../../AGENTS.md).

**Harder page types.** Observe pierces open shadow roots and
same-origin iframes (`pierce: true`). ComboBox / ListBox / Option are
first-class roles. Virtualized lists are the visible window only (scroll
+ re-observe). Autocomplete is TYPE then ticketed option act after
re-observe. After combobox TYPE, read-only option settling is limited to
10 polls at 25 ms intervals and reads its `aria-controls` / `aria-owns`
popup when present. The document fallback ends early only after visible
options change from the first poll and stabilize; input is never
re-dispatched. A plain element the page made clickable (an `onclick`
attribute or handler property, `tabindex` ≥ 0, or a pointer cursor it
sets itself rather than inherits) is offered CLICK, and so is
a plain box Chrome reports `isClickable` (one `DOMSnapshot` call: listeners
added by script). Also kept: SVG shapes (named by title / text / fill; not an icon inside
a click control), `LI` with text,
text-less boxes only with a click hint, `canvas` / `svg` surfaces (claim
`point` only with click evidence), scrollable boxes (claim `scroll`), popup owners (claim
`hover`), and drag-marked elements (claim `drag`). Icon-only controls
are named by title / alt / class or id words plus their row's rendered
text; unrendered text names nothing, and neither does a text area's
own text (its value). Observe
also records control state (value, checked,
expanded, `selected_item` (aria-selected), `aria_current`, select
options) as evidence only: it is not in the region fingerprint, and
password values are never read. Labels drop zero-width characters
(U+200B-D, U+2060, U+FEFF, U+034F, U+00AD; `ultra_instinct_core::INVISIBLE`)
once, as fusion builds each DOM and AX node; every label match (guard
exact label, confirm globs, `browser_wait`) uses
`ultra_instinct_core::normalize_label` on both sides. A label (`for`,
wrapping, or `aria-labelledby` text) folds into the one offered control
it names, in fusion, with compact hit evidence; a zero-size, transparent,
or covered checkbox / radio folds the other way, onto its label's box.
Do not claim cross-origin
iframe or closed-shadow coverage.
