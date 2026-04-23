# Webview XR Native Plan

## Goal

Render the custom webview as native XR primitives instead of flattening the
page into a texture that is later shown on an OpenXR quad.

This plan is intended to be implementation-ready: it captures the current
renderer model, the minimum viable migration path, the files likely to change,
and the checkpoints needed to evaluate progress on-device.

## Why do this first

This should happen before the immersive CSS experiment in
`css-3d-immersive.md`.

Reason:

- the webview already produces structured draw data
- the current panel path throws away that structure at the last stage
- immersive CSS will be cleaner if it selects between **panel-backed** and
  **XR-native** rendering, instead of inventing a second ad hoc render path

## Current state

## What already exists

The webview already does more than paint a single bitmap:

1. DOM + CSS -> styled tree
2. styled tree -> layout tree
3. layout tree -> `DrawCommand`s
4. `DrawCommand`s -> GPU instances
5. GPU instances -> render target
6. on Quest, render target -> compositor quad

The renderer already emits per-element primitives:

- `DrawCommand::Rect`
- `DrawCommand::Border`
- `DrawCommand::Text`
- `DrawCommand::Line`

It also already supports a small CSS 3D subset:

- `perspective(...)`
- `rotateX(...)`
- `rotateY(...)`
- `translateZ(...)`

Those transforms are applied in shader space, together with a depth term based
on `draw_order`.

## What is still missing

The page is still fundamentally panel-space content:

- layout units are CSS/panel pixels
- text is panel text via the glyph atlas path
- hit testing is 2D page hit testing
- the final result is flattened into a texture

So the current renderer is best described as:

> native per-element panel primitives with limited depth, not native XR scene
> primitives

## Desired end state

On XR-capable devices, the webview should be able to render its primitives
directly into XR space.

First target:

- page content still behaves like one page
- elements are rendered as XR-native primitives
- controller rays interact with that page through XR-space hit testing
- the current panel-backed path remains available during migration

Later target:

- selected elements or subtrees can opt into more spatial behavior
- this becomes the substrate for the immersive CSS experiment

## Non-goals for the first implementation

- full browser-grade CSS transform fidelity
- complete support for clipping, overflow, filters, blending, and stacking
  contexts
- replacing WebXR
- removing the existing panel-backed path immediately
- supporting every page in the demo set from day one

## Proposed architecture

Introduce a second rendering IR for XR:

`XrDrawCommand`

Initial shapes:

- `XrQuad`
- `XrBorderQuad`
- `XrGlyphQuad` or `XrGlyphRun`
- `XrLine`

These should be derived from the existing `DrawCommand`s, not from DOM nodes
directly. That keeps the migration incremental and lets the current renderer
continue to serve as the source of truth for layout/styling behavior.

## Coordinate model

Use a page-local XR coordinate system.

Initial assumptions:

- page content stays anchored in the same OpenXR `LOCAL` space as the current
  webview panel
- CSS/layout units are converted into meters using a fixed scale
- page origin and centering rules are explicit and stable
- CSS transform matrices are reinterpreted into page-local XR coordinates rather
  than panel framebuffer coordinates

First version should preserve the feel of the current panel:

- same apparent page size
- same initial position
- same controller interaction semantics

## Rendering model

### Panel-backed path (existing)

- render webview primitives into a texture
- show that texture on the OpenXR quad layer

### XR-native path (new)

- convert `DrawCommand`s into `XrDrawCommand`s
- render XR-native primitives into the eye views directly
- keep interaction in page-local coordinates

During migration, both paths should coexist.

## Interaction model

Hit testing should move to XR space without rewriting DOM interaction:

1. cast controller ray into page-local XR space
2. intersect against the XR-native page plane / primitives
3. convert the hit back into page-local CSS coordinates
4. reuse existing DOM hit testing and event dispatch where possible

This is important: the first version should not invent a new DOM interaction
model. It should primarily replace the rendering backend.

## Text strategy

Recommended first step:

- keep the current glyph atlas approach
- place glyph quads in XR space instead of panel space

Reason:

- least disruptive path
- keeps text shaping/layout logic unchanged
- good enough to validate the XR-native rendering architecture

Possible later upgrades:

- text-run textures
- vector text
- readability-specific XR text treatment

## Implementation phases

## Phase 0: baseline and guardrails

Objective:

- preserve the current Quest panel behavior while building the XR-native path

Tasks:

1. Document the current panel render path and the current XR submission path.
2. Keep the current panel-backed webview as a stable fallback.
3. Avoid changing DOM/layout semantics during this phase.

Success criteria:

- current Quest demo still works
- no regression to pointer, scrolling, or page navigation

## Phase 1: define XR-native IR

Objective:

- create a minimal scene representation for XR-native webview content

Tasks:

1. Add `XrDrawCommand` types.
2. Define page-local units and CSS-pixel-to-meter conversion.
3. Define how draw order maps into XR-native layering.
4. Decide whether the first version uses:
   - true 3D primitive depth, or
   - a page plane plus small depth offsets for deterministic layering

Files likely to change:

- `examples/standalone/04_custom_webview/src/types.rs`
- `examples/standalone/04_custom_webview/src/gpu.rs`
- possibly a new XR renderer module

Success criteria:

- the IR is capable of representing rects, borders, text, and lines
- conversion rules are documented in code comments or adjacent notes

## Phase 2: conversion from existing draw commands

Objective:

- derive XR-native primitives from current renderer output

Tasks:

1. Convert `DrawCommand::Rect` -> `XrQuad`
2. Convert `DrawCommand::Border` -> border geometry/quads
3. Convert `DrawCommand::Text` -> glyph or glyph-run XR primitives
4. Convert `DrawCommand::Line` -> XR line primitives
5. Preserve transform semantics from the current 4x4 matrix path

Files likely to change:

- `examples/standalone/04_custom_webview/src/renderer.rs`
- `examples/standalone/04_custom_webview/src/gpu.rs`
- possibly a new conversion module

Success criteria:

- a representative subset of pages can produce XR-native draw data
- CSS 3D demo transforms survive conversion

## Phase 3: XR-native render pass

Objective:

- render `XrDrawCommand`s directly into XR eye targets

Tasks:

1. Add XR-native pipelines/shaders as needed.
2. Render XR-native primitives into the eye views.
3. Keep the existing compositor quad path available for comparison/fallback.
4. Add a mode switch so flat-panel and XR-native rendering can be compared on
   the same build.

Files likely to change:

- `examples/standalone/04_custom_webview/src/gpu.rs`
- `examples/standalone/04_custom_webview/src/lib.rs`
- `examples/standalone/04_custom_webview/src/xr_session.rs`

Success criteria:

- page content is visible in XR without first being flattened into the panel
  texture
- alignment remains stable during head motion and recentering

## Phase 4: interaction parity

Objective:

- make XR-native rendering behave like the current webview from the user’s point
  of view

Tasks:

1. Raycast into the XR-native page representation.
2. Map hits back into page-local CSS coordinates.
3. Reuse existing event dispatch and scrolling logic.
4. Compare pointer placement and interaction against the panel-backed path.

Success criteria:

- hovering, clicking, and scrolling work
- pointer alignment is stable
- no special-case calibration tied to page size is required

## Phase 5: demo and comparison harness

Objective:

- make the new path easy to evaluate and iterate on

Recommended demo:

- same page rendered twice in XR:
  - left: current flat panel-backed path
  - right: XR-native path

This makes it easier to compare:

- transform correctness
- readability
- aliasing
- pointer interaction
- comfort/depth feel

Suggested page:

- `assets/css3d.html`

Success criteria:

- differences are visually obvious and debuggable
- regressions can be identified quickly on-device

## First implementation slice

To reduce risk, the first slice should be intentionally narrow:

1. Support only:
   - rects
   - borders
   - text
2. Target only one page first:
   - `assets/css3d.html`
3. Keep current page interaction model.
4. Keep panel-backed rendering as fallback.

Do **not** start with arbitrary subtree promotion or a new CSS extension.

## Risks

1. **Text quality risk**
   - XR-native glyph rendering may be harder to read than panel text
2. **Transform mismatch risk**
   - current CSS transform behavior may not map cleanly into XR-native space
3. **Interaction mismatch risk**
   - if hit testing and rendering drift apart, the result will feel broken fast
4. **Renderer duplication risk**
   - if panel-backed and XR-native paths diverge too early, maintenance cost will
     spike

## Mitigations

1. Derive XR-native primitives from existing `DrawCommand`s first.
2. Keep one source of truth for layout and event dispatch.
3. Use side-by-side comparison modes during development.
4. Keep the first supported primitive set small.

## Open decisions to make during implementation

1. Should XR-native webview content still behave like one conceptual page plane
   in v1?
2. Should depth ordering continue to use small deterministic offsets in addition
   to transform-derived depth?
3. How should clipping / `overflow: hidden` be represented in XR-native space?
4. Should text remain glyph-based in v1, or should some elements be promoted as
   textured runs?
5. Should the feature be Quest/XR-only first, or structured as a renderer path
   reusable on desktop later?

## Validation checklist

Use this when implementation begins:

1. Page appears in XR at the expected size and position.
2. CSS 3D cube renders with the same basic transform behavior as the flat path.
3. Pointer hover/click lands where the user expects.
4. Scrolling still works.
5. Recenter/head motion do not introduce drift.
6. Text remains readable enough for the demo page.

## Handoff note

If picking this up later, start with:

1. defining `XrDrawCommand`
2. converting `DrawCommand::Rect` and `DrawCommand::Text`
3. adding a side-by-side comparison mode for `assets/css3d.html`

Only revisit `css-3d-immersive.md` after that path works end to end.
