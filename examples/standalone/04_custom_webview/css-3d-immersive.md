# CSS 3D Immersive Experiment

## Summary

This document proposes an experimental CSS extension for XR-capable webviews:
an element that already uses CSS 3D transforms can opt into being rendered as
immersive spatial content instead of being flattened into a 2D panel.

The goal is not to standardize the open web overnight. The goal is to define a
small, implementable experiment that can be demoed in
`examples/standalone/04_custom_webview`, evaluated on XR hardware, and refined
from there.

## Problem

Today, CSS 3D content in the custom webview is still fundamentally panel
content:

- transforms create a 3D illusion inside the page
- the result is composited back into a flat surface
- XR devices cannot selectively "break out" interesting 3D content into the
  immersive scene

This means authors can build a nice CSS cube, carousel, or stack of cards, but
on an XR device it still behaves like pixels on a screen.

## Goals

1. Let authors opt in per element.
2. Preserve normal web behavior on non-XR or unsupported devices.
3. Reuse existing CSS 3D authoring patterns where possible.
4. Keep the first version small enough to prototype in the custom webview.
5. Provide a clear demo page that compares flat CSS 3D vs immersive CSS 3D side
   by side.

## Non-goals

- full browser standardization
- complete compatibility with arbitrary web content
- perfect support for every CSS transform/perspective edge case
- replacing WebXR
- automatic promotion of all CSS 3D content without author opt-in

## Proposed CSS extension

### Working property name

`xr-transform-mode`

### Initial values

```css
xr-transform-mode: flat | immersive;
```

- `flat`: default behavior; render as normal page content
- `immersive`: if the device and runtime support immersive CSS 3D, render the
  element's 3D subtree as spatial XR content

### Author example

```css
.cube--immersive {
  transform-style: preserve-3d;
  xr-transform-mode: immersive;
}
```

### Fallback behavior

If immersive CSS 3D is unsupported, unknown, disabled, or not in an XR-capable
presentation mode, `xr-transform-mode: immersive` must degrade to `flat`
behavior.

That keeps the page valid and usable everywhere.

## Proposed model

When an element with `xr-transform-mode: immersive` is encountered:

1. The element becomes the root of an **immersive transform subtree**.
2. Descendants continue to use normal CSS transforms, including 3D transforms.
3. Instead of flattening the subtree into the panel texture, the webview
   interprets the subtree as spatial content.
4. The subtree is anchored relative to the webview panel origin, unless later
   experiments define an alternate anchor mode.

For the first prototype, we should only support a narrow subset:

- `transform-style: preserve-3d`
- translate / rotate / scale
- nested transformed elements
- fixed-size positioned elements used as cube faces

## Initial semantics

For v0 of the experiment:

- the immersive subtree is still authored in CSS pixels
- CSS pixel positions are converted into local XR units using a fixed scale
- positive Z from CSS `translateZ()` becomes motion toward the viewer in XR
- the promoted subtree is rendered in the same local XR space as the page panel
- the original flat panel version of that subtree is hidden when immersive mode
  is active

This gives authors a simple rule:

> Build normal CSS 3D content first. Add one CSS property to ask the XR-capable
> webview to spatialize it.

## Suggested optional future properties

These are out of scope for the first prototype, but worth reserving conceptually:

```css
xr-depth-scale: 1;
xr-anchor: panel-local;
xr-backface-visibility: auto;
```

For the first experiment, none of these need to exist yet.

## Rendering behavior

### Flat mode

- existing renderer path
- subtree is rasterized into the page panel

### Immersive mode

- subtree is extracted before flattening
- each participating element becomes XR geometry or an equivalent draw item
- transforms are evaluated from CSS into 3D local space
- visual styling is preserved as much as practical for the prototype

For the first implementation, we should prefer correctness of transform and
placement over full CSS visual fidelity.

## Hit testing

For the first version:

- immersive subtree hit testing should use XR-space geometry derived from the
  promoted elements
- pointer events should map back to the corresponding DOM element
- if immersive hit testing is incomplete, the prototype may start as
  visualization-only for the demo

## Demo plan: `assets/css3d.html`

Modify the existing page to show two cubes:

### Left cube

- existing CSS 3D cube
- remains flat panel content
- label it clearly as `Flat CSS 3D`

### Right cube

- duplicated cube markup and animation
- identical transforms and timing
- add the experimental CSS property
- label it clearly as `Immersive CSS 3D`

Example styling sketch:

```css
.cube--immersive {
  xr-transform-mode: immersive;
}
```

Example structure sketch:

```html
<div class="demo-row">
  <section class="demo-panel">
    <h2>Flat CSS 3D</h2>
    <div class="cube cube--flat">...</div>
  </section>
  <section class="demo-panel">
    <h2>Immersive CSS 3D</h2>
    <div class="cube cube--immersive">...</div>
  </section>
</div>
```

## Implementation plan

### Phase 1: experimental CSS surface

1. Add parsing support for `xr-transform-mode`.
2. Store it in the style data model.
3. Default to `flat`.

### Phase 2: subtree extraction

1. Detect elements marked `immersive`.
2. Identify the promoted subtree root.
3. Prevent that subtree from being doubly rendered into the flat panel.

### Phase 3: transform conversion

1. Reuse existing CSS transform evaluation where possible.
2. Convert CSS transform chains into local 3D transforms for XR rendering.
3. Define a stable CSS-pixel-to-XR-unit scale.

### Phase 4: immersive rendering path

1. Render promoted elements through a dedicated XR draw path.
2. Start with rectangles/textured quads sufficient for the cube demo.
3. Keep the rest of the page in the existing panel path.

### Phase 5: demo page

1. Duplicate the current cube in `assets/css3d.html`.
2. Put flat and immersive variants side by side.
3. Add labels and short explanation text.

### Phase 6: interaction and polish

1. Add hit testing for promoted elements if needed.
2. Validate relative scale and depth feel on Quest.
3. Tune defaults only after the transform pipeline is correct.

## Open questions

1. Should immersive CSS 3D be active only while the page is already presented
   in XR, or also in a 2D panel viewed from XR?
2. Should immersive subtrees stay panel-local, or be allowed to escape entirely
   into world space later?
3. How much CSS visual fidelity is required for the experiment to be useful?
4. Should text remain readable panel text, or be promoted as geometry/quads with
   the rest of the subtree?
5. Do we want a JS capability check in addition to the CSS opt-in?

## Recommendation

Implement the first prototype with:

- one experimental property: `xr-transform-mode: immersive`
- one demo page: duplicated `css3d.html` cube
- one narrow supported subset: promoted transformed rectangles sufficient for
  the cube demo

If the effect is compelling on-device, then we can decide whether to keep
iterating on the CSS property name, broaden the supported CSS subset, or split
the XR-specific logic out of `lib.rs` for maintainability.
