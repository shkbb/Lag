# Vendored crates

## slint-interpreter 1.18.1

An unmodified copy of `slint-interpreter` 1.18.1 from crates.io (licence: GPL-3.0-only OR
LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0, see `LICENSES/`), with ONE addition marked
`LAG PATCH`:

- `highlight::sources_at` + `ComponentInstance::element_sources_at(x, y)`: every element under a point, with the
  source location of each, in one pass over the item tree.

Why: the theme editor (`crates/lag-app/src/theme_editor.rs`) picks the element under the mouse. The public
`component_positions(path, offset)` walks the whole item tree for ONE element (~64 ms on Lag's UI with a full
library), so testing every element of Lag's interface took over a minute per click. The addition reuses the
crate's own private helpers (`all_instances`, `item_flat_index_to_rect`, the LLR debug info).

When upgrading Slint: copy the new version here and re-apply the two `LAG PATCH` blocks (highlight.rs, api.rs),
or drop the patch if upstream exposes an equivalent.
