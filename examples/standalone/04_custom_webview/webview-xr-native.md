# Webview XR Native Plan

## Summary

Before experimenting with immersive CSS extensions, we should first stop
treating the entire XR webview as "a flat page painted onto a quad" and instead
render the page's own native primitives directly into XR space.

This is a better precursor because the current renderer already does a
meaningful amount of structural work:

- it parses DOM and CSS
- computes layout
- emits per-element draw commands
- applies simple CSS 3D transforms in shaders

So the right next step is not "invent immersive CSS from scratch", but:

> promote the existing webview renderer from **panel renderer with 3D-ish
> element transforms** to **native XR renderer of webview primitives**.

## Direct answer to the current question

Yes, mostly: we already construct page content as native draw items, not as a
single bitmap capture of the DOM.

Today the pipeline is roughly:

1. DOM + CSS -> styled tree
2. styled tree -> layout tree
3. layout tree -> `DrawCommand`s
4. `DrawCommand`s -> GPU instances
5. GPU instances -> rendered into a 2D target
6. on Quest, that rendered target is composited onto an OpenXR quad layer

So the current system already has a primitive scene description. But the final
output is still a **flat texture**.

### About the "tiny z offsets" memory

That memory is directionally right, but not the full story.

The current shader path does two things:

- uses a per-instance `draw_order` depth term to keep painter's-order layering
  stable
- also includes transformed Z from the element's 4x4 transform matrix

So elements are not simply stacked with fixed tiny DOM z offsets. Instead:

- regular layering mostly comes from `draw_order`
- CSS transforms like `translateZ`, `rotateX`, `rotateY`, and `perspective`
  contribute real per-vertex depth within the panel render

However, all of this still happens inside the coordinate system of a **2D panel
render target**, not as independent XR-space objects.

## Current renderer behavior

## What already exists

### Per-element primitives

The renderer emits:

- `Rect`
- `Border`
- `Text`
- `Line`

This means the webview already has a native primitive representation for much of
the page.

### CSS transform support

The current CSS engine already parses a small but useful subset:

- `perspective(...)`
- `rotateX(...)`
- `rotateY(...)`
- `translateZ(...)`

Each styled node stores a 4x4 transform matrix.

### GPU transform path

The GPU shaders apply the transform matrix per rect/glyph instance and combine
it with:

- element center / screen placement
- scroll offset
- draw-order depth

That is why the CSS 3D cube demo works at all today.

## What does *not* exist yet

### Native XR scene primitives

The current `DrawCommand`s are still panel-space concepts:

- rects and borders are sized in CSS/layout pixels
- text is rasterized into a glyph atlas for panel rendering
- hit testing is still 2D page hit testing
- output is flattened into the panel texture

### True XR-space composition of page elements

Even when a page uses CSS 3D transforms, the result is not a set of XR quads in
world or local XR space. It is a set of transformed panel-space primitives that
ultimately become pixels on the quad layer.

### Full transform-tree semantics

The current transform propagation is also simplified. It is good enough for the
demo path, but it is not yet a robust retained 3D scene graph for arbitrary
nested transformed DOM content.

## Why this should come before immersive CSS

If we implement `xr-transform-mode: immersive` first, we risk building a second
XR representation path before the base webview renderer is XR-native.

That would likely create:

- one path for flat panel rendering
- one path for special immersive subtree promotion
- duplicated transform interpretation
- duplicated hit testing
- duplicated styling compromises

Instead, we should first give the webview a native XR rendering model. Then the
immersive CSS experiment becomes a smaller question:

> which elements stay panel-flat, and which elements opt into XR-native
> rendering behavior?

## Proposed direction

Create a new intermediate representation for XR-native rendering of page
content.

Working name:

`XrDrawCommand`

Possible early forms:

- `XrQuad`
- `XrBorderQuad`
- `XrGlyphRun`
- `XrLine`

These would be derived from existing `DrawCommand`s but expressed in an XR-ready
coordinate system rather than a panel framebuffer coordinate system.

## Design goals

1. Reuse the existing DOM/CSS/layout pipeline.
2. Reuse the current primitive emission logic where possible.
3. Move from panel-pixel output to XR-space primitive output.
4. Keep flat panel rendering working during migration.
5. Make later immersive CSS subtree promotion a thin opt-in layer, not a second
   renderer.

## Proposed rendering model

### Phase 0: preserve current behavior

Keep the current Quest path working:

- panel swapchain render target
- compositor quad layer
- existing pointer and scrolling behavior

### Phase 1: introduce XR-native IR

Create a parallel representation derived from layout/render data:

- panel-local meters instead of CSS pixels
- explicit 3D transform per primitive
- explicit depth / ordering policy

The first version can still map the whole page onto a single local XR plane if
needed for parity checks.

### Phase 2: render primitives directly in XR

Render page primitives directly into the XR eye views instead of first
rasterizing them into the webview panel texture.

This means:

- page rects become quads in XR space
- borders become line or border geometry in XR space
- text becomes XR-space glyph quads or texture-backed text runs
- 3D CSS transforms affect the XR primitives directly

### Phase 3: separate flat vs spatial behavior

Once the webview renderer itself is XR-native, define which content remains on a
flat local page surface and which content may break spatially away from it.

That is the point where the `css-3d-immersive.md` proposal becomes practical.

## Coordinate model

We need a stable conversion from CSS/layout units to XR units.

Initial proposal:

- define a page-local meter scale
- keep `(0, 0)` at the page origin or page center consistently
- interpret CSS transforms relative to that page-local XR coordinate space
- keep page-local and XR-panel-local space equivalent in the first version

This lets us preserve existing page layout while changing only the final
rendering target.

## Text strategy

Text is the main complication.

Options:

1. **Keep current glyph atlas approach**
   - easiest path
   - glyphs become XR-space textured quads
2. **Promote text runs as offscreen textures**
   - simpler batching
   - lower fidelity for very dynamic text
3. **Use vector/text geometry later**
   - best long-term quality
   - too much scope for the first experiment

Recommendation: start with the existing glyph atlas path and place glyph quads
in XR space.

## Hit testing strategy

Current hit testing is 2D page hit testing.

For XR-native rendering we should:

1. raycast from controller into page-local XR space
2. convert hit position back into page-local CSS coordinates
3. reuse existing DOM hit testing where possible

This keeps interaction logic aligned with rendering without rewriting the DOM
event model first.

## Migration plan

### Step 1: document current renderer assumptions

Record:

- which `DrawCommand`s exist
- which CSS transforms are supported
- how draw order and transformed Z currently interact

### Step 2: define XR-native primitive types

Add a new XR rendering IR without removing the old panel path.

### Step 3: add conversion from `DrawCommand` to `XrDrawCommand`

At first this can support only:

- rects
- borders
- text

That is enough to reproduce a large subset of current demo pages.

### Step 4: render XR-native primitives into the eye views

Do this in parallel with the existing panel path so we can compare output and
interaction.

### Step 5: choose page policy

Decide whether:

- all page content becomes XR-native on XR devices, or
- only selected pages/elements do, while the rest remain panel-backed

### Step 6: revisit immersive CSS

Only after the XR-native page path is working should we implement the
experimental CSS opt-in for selective spatial breakout.

## Demo/use-case for this precursor plan

A good intermediate demo is not yet "immersive cube on the right".

A better precursor demo is:

- render the same page twice in XR
- left: current flat panel path
- right: XR-native primitive path

This would let us compare:

- alignment
- readability
- transform correctness
- pointer interaction
- comfort and depth feel

## Open questions

1. Should XR-native webview rendering still preserve a conceptual page plane, or
   should all elements be allowed to float freely as soon as 3D transforms exist?
2. How should clipping and `overflow: hidden` map into XR-native rendering?
3. How much of current CSS transform behavior is accurate enough to preserve, vs
   rewrite?
4. Should text stay panel-like for readability even if surrounding elements are
   XR-native?
5. Do we want this mode only for XR runtimes, or as a general renderer
   refactor?

## Recommendation

Implement this plan before the immersive CSS property experiment.

Reason:

- the webview already has enough structure to justify XR-native rendering
- the current flat-panel compositing path hides that structure at the last step
- immersive CSS will be much cleaner if it builds on top of an XR-native
  renderer instead of trying to punch holes through a panel-first model
