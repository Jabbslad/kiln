---
version: alpha
name: Prime Intellect
description: Dark infrastructure UI with hairline grids, monospace labels, signal-green accents, a cinematic forest-video hero, LED fields, and technical product figures.
colors:
  primary: "#ffffff"
  on-primary: "#000000"
  secondary: "#202020"
  on-secondary: "#ededed"
  tertiary: "#78e772"
  tertiary-bright: "#85ed75"
  tertiary-glow: "rgba(120, 231, 114, 0.3)"
  tertiary-container: "rgba(94, 233, 181, 0.15)"
  neutral: "#0e0e0e"
  surface: "#0e0e0e"
  surface-container: "#111111"
  surface-container-high: "#151515"
  surface-container-highest: "#1b1b1b"
  surface-footer: "#191919"
  surface-inference: "#101010"
  surface-research: "#071012"
  partner-ramp: "#e1f045"
  surface-bright: "#2c2c2c"
  on-surface: "#f4f4f4"
  on-surface-strong: "rgba(255, 255, 255, 0.9)"
  on-surface-variant: "rgba(255, 255, 255, 0.5)"
  on-surface-muted: "rgba(255, 255, 255, 0.3)"
  on-surface-faint: "rgba(255, 255, 255, 0.25)"
  outline: "#2a2a2a"
  outline-variant: "#202020"
  outline-hover: "rgba(255, 255, 255, 0.28)"
  overlay-2: "rgba(255, 255, 255, 0.02)"
  overlay-4: "rgba(255, 255, 255, 0.04)"
  overlay-6: "rgba(255, 255, 255, 0.06)"
  overlay-10: "rgba(255, 255, 255, 0.10)"
  overlay-16: "rgba(255, 255, 255, 0.16)"
  ring: "rgba(255, 255, 255, 0.6)"
  error: "#ef4444"
  figure-stroke: "#54565a"
  figure-stroke-dim: "#404143"
  figure-stroke-faint: "#2c2d2e"
  figure-fill: "#1a1a1a"
  figure-fill-light: "#cbcccd"
  figure-fill-mid: "#b1b3b6"
  figure-fill-dark: "#6a6c72"
  glow-green: "rgba(125, 255, 166, 0.17)"
  glow-cyan: "rgba(73, 220, 255, 0.1)"
  matrix-dot: "#1a1a1a"
  chart-1: "#85ed75"
  chart-2: "#f3bc56"
  chart-3: "#8b7cf6"
  chart-4: "#38bdf8"
  chart-5: "#fb7185"
typography:
  display:
    fontFamily: Geist
    fontSize: 40px
    fontWeight: 400
    lineHeight: 1.1
    letterSpacing: 0em
  headline-lg:
    fontFamily: Geist
    fontSize: 36px
    fontWeight: 400
    lineHeight: 1.1
    letterSpacing: 0em
  headline-md:
    fontFamily: Geist
    fontSize: 28px
    fontWeight: 400
    lineHeight: 1.22
    letterSpacing: 0em
  headline-sm:
    fontFamily: Geist
    fontSize: 20px
    fontWeight: 400
    lineHeight: 1.5
    letterSpacing: 0em
  body-lg:
    fontFamily: Geist
    fontSize: 18px
    fontWeight: 400
    lineHeight: 1.5
  body-md:
    fontFamily: Geist
    fontSize: 16px
    fontWeight: 400
    lineHeight: 1.5
  body-sm:
    fontFamily: Geist
    fontSize: 14px
    fontWeight: 400
    lineHeight: 1.43
  body-xs:
    fontFamily: Geist
    fontSize: 12px
    fontWeight: 400
    lineHeight: 1.3
  label-lg:
    fontFamily: ABC Favorit Mono
    fontSize: 14px
    fontWeight: 400
    lineHeight: 1
    letterSpacing: 0em
  label-md:
    fontFamily: ABC Favorit Mono
    fontSize: 12px
    fontWeight: 500
    lineHeight: 1
    letterSpacing: 0em
  label-sm:
    fontFamily: ABC Favorit Mono
    fontSize: 10px
    fontWeight: 400
    lineHeight: 1
    letterSpacing: 0em
  code:
    fontFamily: Geist Mono
    fontSize: 12px
    fontWeight: 400
    lineHeight: 1.5
rounded:
  none: 0px
  sm: 2px
  md: 4px
  full: 9999px
spacing:
  unit: 4px
  xs: 4px
  sm: 8px
  md: 16px
  lg: 20px
  xl: 32px
  2xl: 40px
  3xl: 70px
  4xl: 80px
  page-margin: 16px
  page-margin-md: 20px
  container-max: 1440px
  content-max: 1024px
  header-height: 64px
  announcement-height: 36px
  hero-height: 528px
  hero-height-small: 680px
  hero-height-md: 640px
  lab-banner-height: 320px
  compute-banner-height: 360px
  figure-height: 250px
  figure-height-md: 300px
  figure-height-lg: 402px
  breakpoint-sm: 640px
  breakpoint-md: 768px
  breakpoint-lg: 1024px
  breakpoint-xl: 1280px
  control-height: 28px
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.on-primary}"
    typography: "{typography.label-md}"
    rounded: "{rounded.none}"
    height: "{spacing.control-height}"
    padding: 8px
  button-primary-hover:
    backgroundColor: "#e6e6e6"
    textColor: "{colors.on-primary}"
  button-secondary:
    backgroundColor: "{colors.overlay-10}"
    textColor: "rgba(255, 255, 255, 0.85)"
    typography: "{typography.label-md}"
    rounded: "{rounded.none}"
    height: "{spacing.control-height}"
    padding: 10px
  button-secondary-hover:
    backgroundColor: "{colors.overlay-16}"
    textColor: "{colors.primary}"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.on-surface-variant}"
    typography: "{typography.label-md}"
    rounded: "{rounded.none}"
    height: "{spacing.control-height}"
    padding: 8px
  button-ghost-hover:
    backgroundColor: "{colors.overlay-16}"
    textColor: "{colors.primary}"
  button-link:
    backgroundColor: "transparent"
    textColor: "{colors.primary}"
    typography: "{typography.label-md}"
    padding: 0px
  card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.on-surface}"
    rounded: "{rounded.none}"
    padding: "{spacing.lg}"
  card-hover:
    backgroundColor: "{colors.overlay-2}"
  card-raised:
    backgroundColor: "{colors.surface-container}"
    textColor: "{colors.on-surface}"
    rounded: "{rounded.none}"
    padding: "{spacing.lg}"
  chip:
    backgroundColor: "{colors.overlay-6}"
    textColor: "{colors.on-surface-variant}"
    typography: "{typography.label-sm}"
    rounded: "{rounded.none}"
    padding: 4px
  chip-active:
    backgroundColor: "{colors.overlay-10}"
    textColor: "{colors.primary}"
  chip-status-available:
    backgroundColor: "{colors.tertiary-container}"
    textColor: "{colors.tertiary-bright}"
  input:
    backgroundColor: "{colors.overlay-4}"
    textColor: "{colors.on-surface}"
    typography: "{typography.body-sm}"
    rounded: "{rounded.none}"
    height: 36px
    padding: 8px
  figure-label:
    backgroundColor: "transparent"
    textColor: "{colors.on-surface-faint}"
    typography: "{typography.label-sm}"
  code-block:
    backgroundColor: "{colors.surface-container}"
    textColor: "rgba(255, 255, 255, 0.75)"
    typography: "{typography.code}"
    rounded: "{rounded.none}"
    padding: "{spacing.md}"
  tooltip:
    backgroundColor: "{colors.overlay-6}"
    textColor: "{colors.on-surface}"
    typography: "{typography.label-sm}"
    rounded: "{rounded.md}"
    padding: 4px
  partner-quote:
    backgroundColor: "{colors.partner-ramp}"
    textColor: "{colors.on-primary}"
    rounded: "{rounded.none}"
    padding: "{spacing.lg}"
  footer:
    backgroundColor: "{colors.surface-footer}"
    textColor: "{colors.on-surface}"
  divider:
    backgroundColor: "{colors.outline-variant}"
    height: 1px
  header:
    backgroundColor: "transparent"
    textColor: "{colors.on-surface}"
    height: "{spacing.header-height}"
  figure-illustration:
    backgroundColor: "transparent"
    textColor: "{colors.figure-fill-light}"
    typography: "{typography.label-sm}"
  figure-label-chip:
    backgroundColor: "{colors.figure-fill-dark}"
    textColor: "{colors.figure-fill-light}"
    typography: "{typography.label-sm}"
    rounded: "{rounded.none}"
    padding: 2px
  background-matrix:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.matrix-dot}"
---

# Prime Intellect Design System

## Overview

Prime Intellect is an infrastructure company for open superintelligence: compute, RL environments, evaluations, hosted training, and inference. The marketing UI should feel like a **precision instrument** — closer to a well-designed terminal, a lab notebook, or an engineering schematic than a generic consumer landing page.

The look is **near-black, hairline-bordered, and typographically restrained**. Most UI uses white at varying opacities on a `#0e0e0e` ground. Phosphor green (`#78e772`) marks prompts, live status, chart readouts, and the hero headline glow. This is a UI-accent rule, not an image restriction: the hero has natural forest greens, blue atmospheric light, and warm reflections; LED assets retain RGB subpixels; Ramp's testimonial has a yellow-lime branded fill.

Structure comes from **lines, not fills**. Sections are boxed in 1px `#202020` borders, except the full-bleed hero and footer. Product figures use `FIG.1`, `01`, `1.1`, and `1.2` as engineering annotations. Most labels are uppercase monospace; headlines and body copy use a clean grotesque. Controls are square, while diagrams may contain circles, arcs, and curves.

Atmosphere comes from a **forest/portal video hero**, perspective **dot-matrix / LED fields**, technical schematics, and embedded product demonstrations. Do not replace these distinct families with a generic green gradient.

Target audience: ML researchers, infrastructure engineers, and technical founders. The emotional target is **quiet confidence** — serious, dense, legible, and entirely free of ornament. If a choice feels "designed", strip it back.

### Reference scope and fidelity

Reference: [Prime Intellect homepage](https://www.primeintellect.ai/), inspected in Chromium at **1440×1000** and **390×844**, including scrolling to load imagery and opening the mobile menu. Snapshot deployment: `dpl_FdZWUWWeCFpfyHKAvRE5NC8BrGqe`. Editorial content, careers counts, demos, and media may change after this snapshot.

Format: [Stitch DESIGN.md specification](https://stitch.withgoogle.com/docs/design-md/specification.md), alpha. That URL served an application shell during review; schema and section order were checked against Google's [canonical specification source](https://github.com/google-labs-code/design.md/blob/main/docs/spec.md). YAML values are normative. Borders, media URLs, transforms, breakpoints' application rules, and motion live in prose; they are not unsupported top-level token groups. Border rules are deliberately in prose rather than the non-standard `borderColor` component property.

This file specifies the **marketing homepage style**, not the authenticated app or blog theme. Existing generic form/tooltip rules are implementation guidance, not evidence that those controls occur on the homepage. Exact images, brand marks, and vector geometry require the referenced assets; prose or image-generation prompts alone cannot reproduce them. Download authorized copies into the implementing project and pin them; this document does not bundle media or guarantee permanent remote availability. Static fallbacks and accessibility improvements below are explicitly distinguished from observed behavior. Pixel-exact animation and editorial-copy parity require a separately pinned implementation/reference capture.

## Colors

The core UI palette is a monochrome ramp with one accent. Core UI greys are neutral (the research-media ground is an explicit cool-tinted exception). Most "colours" in the UI are white at a specific alpha, layered over the ground.

- **Primary — White (#ffffff):** Solid primary CTA fill and explicitly white text. `on-surface` remains soft white `#f4f4f4`; the hero title specifically uses 90% white. White on the ground has approximately 19.3:1 contrast.
- **On Primary — Black (#000000):** Text on white buttons and the Ramp quote.
- **Secondary — Graphite (#202020):** The structural grey. Used for hairline section borders and secondary surfaces. Not a text colour.
- **Tertiary — Phosphor Green (#78e772):** The main chromatic UI accent. Reserved for the terminal prompt, hero glow, live/available status, and primary chart series. A brighter variant (#85ed75) is used for solid status dots. Never use it for buttons, links, or large fills.
- **Neutral / Surface — Void (#0e0e0e):** The page ground. Slightly lifted from pure black so borders and overlays register.
- **Surface Container (#111111) / High (#151515) / Highest (#1b1b1b):** Progressive tonal lifts for panels, cards, popovers, and code blocks. The lift between steps is deliberately tiny — hierarchy should be felt, not seen.
- **Outline (#2a2a2a) / Outline Variant (#202020):** Border colours. `#202020` is the default hairline; `#2a2a2a` is used for inputs and raised cards. On hover, borders brighten to `rgba(255,255,255,0.28)`.
- **White alpha ramp:** Text and fills use white at fixed opacities. Text: 90% (emphasised), 100%/#f4f4f4 (default), 50% (secondary), 30% (tertiary, e.g. hero kicker), 25% (figure labels), 20% (decorative). Fills: 2% (card hover), 4%, 6% (chips, default overlays), 10% (secondary buttons), 16% (secondary button hover).
- **Error — Signal Red (#ef4444):** Destructive actions and validation states only.
- **Chart series:** Green (#85ed75), amber (#f3bc56), violet (#8b7cf6), sky (#38bdf8), rose (#fb7185) are available for new multi-series charts. The homepage reward curve is **white** with a green numeric score; evaluation values also use red and amber. Do not recolor these examples to fit a generic series rule.
- **Explicit surface exceptions:** Footer `#191919`, inference panels `#101010`, research feature `#071012`, and Ramp quote `#e1f045` with black text. Raster/video colors are intrinsic to their assets, not additions to the control palette.

There is no light theme for the product surfaces. (The blog supports a light mode with `#fafaf8` paper and `#24231f` ink, but that is out of scope for this system.)

## Typography

Two typefaces carry the whole system, plus a monospace for code.

- **Geist (sans):** Main marketing headlines, body, and UI copy. Use **Regular (400)**; Medium (500) is permitted for button labels and inline emphasis. Avoid 600+ weights in marketing copy. Some embedded demos and the hiring block inherit system sans in the inspected site; Geist is the portable default, not a claim that every rendered glyph uses it. Letter-spacing is normal — do not tighten headlines.
- **ABC Favorit Mono (labels):** Marketing eyebrows, buttons, figure numbers, nav items, and metadata. Use **uppercase**, line-height 1, no added tracking (the mono spacing provides the rhythm). Sizes are small: 10px for figure labels (`FIG.1`), 12px for buttons and nav, 14px for hero kickers. Fallback: `monospace`.
- **Geist Mono (code):** Terminal output and code blocks. 12px, 75% white by default. The hero install command is 14px, 46% white, with only `$` green. Never uppercase commands, identifiers, mixed-case model names, or product-demo content.

Scale:

- **Display (40px / 1.1):** Page-level titles on inner pages. Single line where possible.
- **Headline LG (36px / 1.1):** Hero headline ("Own Your Intelligence"). Rendered at 90% white with a faint green text-shadow (`0 0 44px rgba(120,231,114,0.2)`). Keep this headline effect localized; prompt and chart emphasis have their own subtle glow treatments.
- **Headline MD (28px / 1.22):** Section titles ("Join Prime Intellect").
- **Headline SM (20px / 1.5):** Numbered feature titles ("01 RL Environments"). The number is set in the same face at 50% opacity.
- **Body LG / MD / SM (18 / 16 / 14px):** Descriptive copy at 50% white; primary copy at 100%. Body line-height 1.5; 14px body may use 1.43.
- **Body XS (12px / 1.3):** Dense card descriptions, feature sub-points (`1.1`, `1.2`). Frequently at 50% white with line-clamp.
- **Label LG / MD / SM (14 / 12 / 10px):** Favorit Mono, uppercase. LG for hero kicker at 30% white, MD for buttons and nav, SM for figure annotations at 25% white.

Hierarchy is achieved primarily through **size and opacity**, not boldness.

### Font acquisition

Load fonts before judging line breaks. Verified reference URLs (Latin subsets for Geist):

- Geist variable, weight 100–900: `https://www.primeintellect.ai/_next/static/media/22a5144ee8d83bca-s.p.woff2`.
- Geist Mono variable, weight 100–900: `https://www.primeintellect.ai/_next/static/media/7d4881bb7e1bf84d-s.p.woff2`.
- Favorit Mono 400: `https://www.primeintellect.ai/fonts/ABCFavoritMono-Regular-Trial.woff2`.
- Favorit Mono 500: `https://www.primeintellect.ai/fonts/ABCFavoritMono-Medium-Trial.woff2`.

Use `@font-face`, normal style, the correct weight/range, and `font-display: swap`. Production should use appropriately licensed font distributions rather than relying on hashed deployment URLs or trial fonts. Geist/Geist Mono can be obtained from their official distributions; ABC Favorit requires checking its license. A deliberate fallback is `"Geist Mono", monospace` for labels and `Arial, sans-serif` for Geist; this changes metrics and is **not** an exact match. Do not synthesize Medium from Regular when matching the reference.

## Layout

The layout is a **full-bleed, bordered-grid** model. The page is a single column of stacked, bordered sections; inside each section, content splits into hairline-separated cells.

- **Container:** Full width up to `1440px` (`max-w-360`), centred, with `16px` page margins on mobile and `20px` from `md` up. Long-form content caps at `1024px`.
- **Header:** Fixed `64px` navigation row. At `xl`, a `36px` announcement strip offsets the row downward at the top of the homepage: total visible header footprint `100px`, not `64px`. Logo left, numbered product nav centre, utility links and Login/Start Training right. Below `xl`, use the mobile menu instead.
- **Sections:** Each major section is a `border border-[#202020]` box. Sections are separated by bottom margin of `20px` (mobile) → `32px` (md) → `70–80px` (lg). Some sections carry a subtle vertical gradient from `#111111` to `#0e0e0e`.
- **Cells:** Content inside a section is divided by shared 1px borders (`border-l`, `border-t`) rather than gaps — partner logos, case studies, and feature panels tile edge-to-edge like a table. Cells use `20px` internal padding.
- **Spacing scale:** Base unit is `4px` (Tailwind `--spacing: 0.25rem`). Common steps: 4, 8, 16, 20, 32, 40, 70, 80. Card padding is `20px`; feature lists use `20px` gap on mobile and `32px` on desktop.
- **Numbering:** Features and sub-features use `01`, `1.1`, `1.2`, `1.3`; Lab figures use `FIG.1`…`FIG.4` at top-left. Do not automatically number unrelated footer, editorial, or navigation lists.
- **Responsive:** `sm = 640px`, `md = 768px`, `lg = 1024px`, `xl = 1280px`; hero media additionally changes at `560px`. Lab is **one column → two columns at lg**, never a four-up band. Inference service cards are one column → three at `lg`. The partner grid remains five columns even on mobile. Detailed exceptions follow.

### Homepage composition

Preserve this order. Dimensions below are CSS pixels; fixed-height media stages must not become fixed-height text containers.

| Block | Desktop composition | Narrow-screen behavior |
| --- | --- | --- |
| Hero | Unbordered, full-viewport media; 640px stage at `md+`. Copy is bottom-left, backers bottom-right at `xl`. 20px side padding, 40px bottom padding. Headline 36px/1.1; description 18px/27px at white/45, max-width 480px, 12px top margin. CTA pair after 24px; install line after 24px. | 680px stage at widths ≤560px, 528px at 561–767px. Copy then backers stack, with 40px margin at ≤560px and 48px otherwise. Title naturally wraps to two lines at 390px. Do not center the copy. |
| Partner grid | Immediately below hero: ten logos in two rows of five, 100px-high cells, shared hairlines, 10px padding. Logo height max 24px. Hero-to-Lab gap 80px at `lg`. | Keep five columns, scale logos with `object-fit: contain`, hide CTA badge words below `md`, retain tiny arrow. Hero-to-Lab gap 20px. |
| Lab | 320px image banner; 2×2 figure grid at `lg`, ordered RL Environments, Evaluations, Hosted Training, Inference. Each figure stage is 402px high, preceded by a small figure-label strip. Copy below, not beside the illustration. Within the copy area, summary and numbered subfeatures sit side by side on wide screens. | Single-column figures; stage 250px, then 300px at `md`; padding 16px then 20px at `md`. Summary and subfeatures stack. Keep diagrams contained or intentionally clipped within their stage, never stretch their aspect ratio. |
| Testimonials | After Lab figures, gap 24/32/40px by base/md/lg. Ramp quote and Zapier quote: `2fr minmax(360px,1fr)` at `xl`, at least 300px high. | Stacked quotes. Let the long Ramp quote increase height. |
| Environment Hub | Same 24/32/40px gap. At `lg`, 443px-high bordered split, 524px-wide copy region left and clipped app preview right. Below it, a three-column code-example band. | Copy then preview; code cards stack. Horizontal overflow is confined to code/demo viewports. |
| Inference | Header padding 20px; inner minimum height 240/280/300px at base/md/lg (outer roughly 280/320/340px plus border). Overview split `0.92fr 1.08fr` at `lg`, min-height 420px. Three service cards below, min-height 544px, 32px padding. Final feedback-loop row `0.95fr 1.05fr`, min-height 384px. | Overview, services, and feedback row stack. Keep illustration above copy in the feedback row. Do not crop schematic labels. |
| Compute | 360px LED banner; two min-height 606px rows at `lg`. First row: On demand copy left, GPU table right. Second: reserved-capacity demo left, Liquid Reserved Clusters copy right. | First row copy → demo; second row demo → copy. Retain local clipping/scaling inside demo stages. |
| Research | 20px-padded heading/CTA, then a large media feature left and compact article list right at `lg`. Latest Research heading and three bordered article tiles below (three columns from `md`). | Stack feature, article list, and tiles. Featured media min-height 380px, 480px from `md`. |
| Customer Stories | Bordered heading and two 490px-high cards: Zapier has image artwork; Ramp is dark with a logo and title. At `lg`, the inactive card has a 370px basis and the active card fills remaining width; emphasis changes with a 500ms flex transition. | Cards stack vertically (`flex-direction:column`). The reference's `flex:1` can compress nominal heights; for a robust adaptation use content-safe `min-height` and retain both stories rather than copying accidental clipping. Do not turn this into an invented carousel. |
| Hiring | 20px-padded bordered split at `lg`, text max-width 540px, team photo right. Text area min-height 377px at `lg`. | Text then photo; photo stage 200px, 280px at `md`. |
| Footer | Full-bleed `#191919`; inner max-width 1400px, 40px horizontal padding. Brand/interactive type and CTA pair left (~2/3), grouped links right. Top padding 32/56/80px at base/md/lg. | Stacked groups; decorative canvas hidden below `md`. Keep text link groups and CTAs. |

Hero copy for a reference build: kicker `THE OPEN SUPERINTELLIGENCE STACK`; title `Own Your Intelligence`; description `Train, deploy, and continuously improve your own models on an integrated compute, training, inference, and sandbox stack.`; CTA pair START TRAINING / BOOK A CALL; command `$ pip install prime`. Backers are a 14px sans text list with slash separators: Founders Fund / Radical / NVIDIA / Intel / Andrej Karpathy / John Schulman / Dylan Patel / Clem Delangue. Use white/54 for `Backed by`, white/62 for names, white/30 for slashes, and wrap the list on narrow screens.

At 1440px viewport width, bordered sections span **x=20 to x=1420**. At 390px, they span **x=16 to x=374**, while hero and partner-grid padding stays 20px. Use `width:100%; max-width:1440px; margin-inline:auto; padding-inline:16px` on the outer wrapper, switching padding to 20px at `md`, with `box-sizing:border-box`; do not add the padding outside the max-width. The hero escapes the wrapper's padding; its media can span the viewport. Anchor targets use approximately 68px scroll margin, 104px at `xl`.

## Elevation & Depth

The system is **flat**. Depth is communicated by borders and tonal lifts, not shadows.

- **Borders first:** Major content panels are defined by a 1px `#202020` border. Adjacent cells share borders; never double them up.
- **Tonal lift:** Raised elements step from `#0e0e0e` → `#111111` → `#151515` → `#1b1b1b`. Each step is barely perceptible; that is intentional.
- **Hover states lift by alpha:** Cards go to `rgba(255,255,255,0.02)`, borders to `rgba(255,255,255,0.28)`, text from 50% → 70% white.
- **Glass:** Secondary CTAs use `backdrop-blur-md` (12px), `bg-white/10`, and `border-white/16`, including in the footer. Header product tiles use white/25, and the announcement strip also uses blur. Do not apply glass to whole content sections.
- **Shadows:** Almost none. The three inference service cards and some overlays use `inset 0 1px 0 rgba(255,255,255,0.045), 0 24px 60px rgba(0,0,0,0.18)`. Preserve that subtle exception; do not add obvious floating-card shadows elsewhere.
- **Glow:** Keep luminous effects localized: `0 0 18px rgba(120,231,114,0.3)` on `$`, `0 0 24px rgba(120,231,114,0.2)` on the hero command line, and `0 0 44px rgba(120,231,114,0.2)` on the hero title. Chart emphasis may glow faintly. Background washes sit behind content with their specified screen/lighten blend; never add a glowing outline to every card.

## Shapes

The shape language is **strictly rectilinear**. The Tailwind radius base is `--radius: 0px`, so `rounded-sm`, `rounded-md`, `rounded-lg`, and `rounded-xl` all resolve to square corners.

- **Containers, cards, sections, buttons, inputs:** `0px`. Square.
- **Tiny utility elements** (status dots, tooltips, tiny keyboard-hint chips): may use `2px`–`4px`, or `full` for dots only.
- **Illustrations and figures:** Line-drawn and schematic, with strokes often brighter than container borders. Circles in the RL workflow, globe arcs, node dots, and triangular feedback loops are intentional; the square-corner rule applies to UI containers, not diagram geometry.

If you are reaching for a rounded corner, you are probably wrong.

## Components

### Imagery & Backgrounds

Media is essential, not optional decoration. The homepage combines **cinematic video**, **raster LED fields**, **vector/HTML product figures**, **partner artwork**, and a **team photo**. Replacing the hero with `lab.png`, a CSS dot grid, or an AI-generated green background is not a faithful recreation.

#### Style references

The umbrella style is **technical minimalism with hardware-macro imagery**: Swiss-style typographic restraint, explanatory schematic figures, and backgrounds that depict the physical substrate of computing (LEDs, phosphor, dither) rather than abstract 3D renders or gradients. It belongs to the current developer-infrastructure design cohort (Vercel, Modal, Linear, Zed, Resend, Stripe's system diagrams).

- **Background fields → "subpixel / LED-matrix macro".** Macro photography of a display panel: RGB subpixel triads on black, chromatic fringing, phosphor-green highlights. Related vocabulary: *CRT / phosphor aesthetic*, *perspective pixel grid* (the Tron/synthwave infinite grid stripped of neon saturation — datacentre indicator lights, not a sunset), *ordered dither / halftone / ASCII shading* for the green burst variants, *cyber-industrial*, *hardware-brutalist*.
- **Inference figures → "isometric engineering schematic".** 30° axonometric line drawings in the lineage of *patent drawings*, *exploded-view diagrams*, and *Dieter Rams / Teenage Engineering instruction-manual line art*; *blueprint / wireframe* with dark-grey fills on black instead of white paper. Diagrams explain a data flow; they never decorate.

Useful prompt terms when generating new assets: `macro photo of LED panel subpixels, black background, perspective, phosphor green trace`, `ordered dither, 1-bit, green on black, radial burst`, `isometric technical schematic, 1px grey line art, dashed projection lines, monospace uppercase callout labels, dark background`.

#### Background fields (raster)

Reference assets are listed with resolvable sources below. These are **section** backgrounds, not the homepage hero.

- **Subject:** A **dot-matrix / LED-panel field** seen in perspective — a plane of tiny dots receding to a vanishing point, as if looking across the surface of a display or a datacentre wall of indicator lights. The dots are rendered as **RGB subpixel triads** (red-green-blue slivers), so the field reads as chromatic grey up close and neutral from a distance.
- **Variants:**
  - *Lab banner:* a curving RGB dot grid on black, dense toward the bottom/right. Preserve the native chromatic fringing rather than painting green traces on top.
  - *Compute fields:* vertical **RGB-striped bars** in strong perspective, converging left to right, like a corridor of server LEDs.
  - *Inference fields:* a **green ASCII-dither burst** — halftone/dot-shaded rays radiating from an off-centre point, monochrome green on black.
  - *Static texture:* a quiet **uniform dot grid** (`#1a1a1a` on `#0e0e0e`, ~24px pitch) with a slight warp, used as a neutral backdrop for figures.
- **Colour:** Black ground with intrinsically colored RGB subpixels; inference is predominantly green/cyan. Do not desaturate these assets or impose a global green tint.
- **Application:** Decorative layers are absolute, clipped by a relative `overflow:hidden` section, `pointer-events:none`, non-selectable, and hidden from accessibility APIs. Content stays at `z-index:10`. Use the per-asset blend/overlay recipes below, **not** one universal `mix-blend-lighten` rule.
- **Glow washes (CSS):** The inference banner adds radial gradients over its raster; they do not replace it. A quiet generated dot texture can be an explicitly approximate fallback, never a substitute for the hero or branded figures.

#### Figures (vector)

Isometric references: `inference-stack.svg`, `dedicated-inference.svg`, `lora-hot-swapping.svg`, and `serverless-api.svg`. **`rl-environments.svg` is a flat diamond workflow with four overlapping circles**, not an isometric stack. Other figures include an evaluation table, reward chart, globe, and triangular training loop. Keep their distinct geometry.

- **Subject:** **Isometric technical schematics** — stacked planes, cubes, grids, and connectors drawn at a 30° isometric projection, explaining a data flow (traces → response, prompt → routing → adapter). They read like an engineering exploded-view or a patent drawing, not an icon.
- **Line work:** 1px strokes in `#54565a` (primary), `#404143` (secondary), `#2c2d2e` (faint), with `stroke-linejoin: bevel`. Dashed strokes denote projection/guide lines; solid strokes denote real edges. Arrowheads are small, filled, and unadorned.
- **Fills:** Dark planes `#1a1a1a` at 50–100% opacity; highlighted planes `#cbcccd` / `#b1b3b6`; small square node markers in `#cbcccd`. Green `#85ed75` is used for at most a handful of nodes or an active edge per figure.
- **Labels:** Uppercase monospace (ABC Favorit Mono, ~9–10px) inside small square chips — `#6a6c72` fill with `#cbcccd` text — attached to the element they name (`TRACES`, `PROMPT`, `LORA ADAPTER`, `ROUTING`). Short horizontal tick marks in the margins act as leader/reference lines.
- **Application:** Use `object-fit:contain`, preserve `viewBox` and intrinsic proportions, and center figures in their cells. Lab figure labels belong above the stage, top-left. Inference service diagrams align toward the card bottom. Preserve SVG-defined strokes, fills, and label geometry rather than forcing all assets to one CSS stroke color.

#### Product screenshots

App previews are a mixture of HTML and inline SVG, not a set of downloadable screenshots. Use near-black panels, tiny 10–14px text, thin dividers, clipped scroll regions, and no device frame. Preserve real text and controls where practical; don't turn all previews into generic code windows.

#### Source asset manifest

**Resolution rule:** Every path below resolves against **`https://www.primeintellect.ai`**, e.g. `https://www.primeintellect.ai/backgrounds/lab.png`. These are remote reference paths, **not files already present in this repository**. Core media/font endpoints were checked successfully during review. Omit the deployment query string and Next.js image optimizer wrapper when acquiring originals. Prefer authorized local copies retaining these paths; record checksums when creating an offline bundle. URLs are provenance, not a grant of image, trademark, or font rights. If an asset cannot be obtained, report the substitution rather than silently claiming fidelity.

| Role | Original path | Intrinsic size / application |
| --- | --- | --- |
| Hero video | `/backgrounds/pi-glass-loop-prod.webm`, fallback `/backgrounds/pi-glass-loop-prod.mp4` | WebM ~3.7MB, MP4 ~7.7MB; forest gorge, reflective water, glowing glass portal. Use the crop recipe below. |
| Hero poster | `/backgrounds/pi-glass-loop-poster.webp` | 1920×1080; same composition as video. Required static/reduced-motion fallback. |
| Lab banner | `/backgrounds/lab.png` | 4200×1052; cover, center; screen blend over surface. |
| Compute banner | `/backgrounds/compute-bg.png` | 4200×1080; cover, center; lighten blend. |
| Inference banner | `/backgrounds/inference-header.png` | Green dither/scan-line burst; cover, center, normal blend over black plus specified overlays. |
| Reserved-capacity stage | `/backgrounds/fig-7-bg.png` | 2100×1818; cover, center, lighten blend behind the HTML capacity card. |
| Hub texture | `/backgrounds/environment-hub-bg.png` | 863×443; contain, left aligned, translate X −20px behind Hub copy/demo. |
| RL workflow / FIG.1 | `/backgrounds/rl-environments.svg` | 456×313; INIT top, DEVELOP right, EVAL bottom, PUSH left, faint circles and a connected diamond. |
| Inference overview | `/backgrounds/inference-stack.svg` | 429×417; contained inside square max-width 480px panel, background `#111`, 32px padding. |
| Dedicated diagram | `/backgrounds/dedicated-inference.svg` | viewBox 259×353; displayed max-width 210px, opacity .9. |
| LoRA diagram | `/backgrounds/lora-hot-swapping.svg` | viewBox 414×350; contained, opacity .9; retain the exploded planes and central white node. |
| Serverless diagram | `/backgrounds/serverless-api.svg` | viewBox 273×288; contained, opacity .9; converging curves above a plane. |
| Ramp testimonial | `/backgrounds/ramp-bg.svg` | 930×301; cover, bottom right. Includes `#e1f045` fill and oversized outline mark; black quote text. |
| Zapier story | `/backgrounds/zapier-story-bg.png` | 3840×2162; cover, bottom left, scale 2.5 with bottom-left transform origin. |
| Hiring photo | `/backgrounds/primeintellect-team.png` | Decoded size 1539×622 in this snapshot; contain, centered on mobile, right aligned at `lg`. Do not crop faces. |
| Research feature | `/blog/prime-agent/onboarding-splash.gif` | Animated raster; cover, center, scale 1.16, translate Y −36px, brightness 1.1. Base opacity .9 → 1 on hover; dark bottom/left overlays on `#071012`. |
| Header brand | `/icons/primeintellect-logo.svg` | Combined icon and wordmark, displayed 32px high, automatic width. Never recreate with typed text. |
| Footer brand | `/icons/primeintellect-text-logo.svg` | Wordmark, reference display approximately 169×17px. |

Partner-grid assets, in row order, use `/icons/partners/` plus: `ramp.svg`, `nvidia.svg`, `zapier.svg`, `characterai.svg`, `goodfire.svg`, `inception.svg`, `arcee.svg`, `browserbase.svg`, `flapping-airplanes.svg`, `standard-intelligence.svg`. Preserve their native grayscale; don't substitute text logos. The Flapping Airplanes cell also references `airplane-white-cropped.gif` for its animated treatment. Testimonial/story variants are `ramp-dark.svg`, `ramp-text-light.svg`, and `zapier-light.svg` in that same folder.

Utility assets live under `/icons/`: `arrow-up-right.svg`, `arrow-right-white.svg`, `copy.svg`, `explore.svg`, `star.svg`, `lock.svg`, `check-green-circled.svg`, `search.svg`, `caret-down.svg`, `single-node-active.svg`, `multi-node.svg`, `nvidia.svg`, `vm.svg`, and `python.svg`. Use SVG/currentColor line icons at 12–16px; do not introduce large filled icon badges. The primary CTA's chevron is a 12×12 SVG, path `M4.75 9.125L7.875 6L4.75 2.875`, no fill, 1px currentColor stroke, square caps.

#### Hero media and layering recipe

The stage is relative, clipped, full viewport width; decorative media sits beneath copy and navigation. Video attributes: `autoplay muted loop playsinline`, with WebM first, MP4 second, and the poster above. The reference preloads video; an adaptation may prioritize the poster and defer video on slow connections.

Use `position:absolute; inset:0; width:100%; height:100%; object-fit:cover; transform-origin:center`. The image is **horizontally mirrored**. Apply CSS individual transforms (`translate`, then `scale`) as follows; do not omit mirroring or approximate this with a generic centered cover crop:

| Viewport | Stage height | `object-position` | `translate` | `scale` |
| --- | --- | --- | --- | --- |
| ≤560px | 680px | 78% 78% | 80px −40px | −1.45 1.45 |
| 561–767px | 528px | 90% 90% | 144px −88px | −1.6 1.6 |
| 768–1279px | 640px | 90% 90% | 144px −88px | −1.6 1.6 |
| ≥1280px | 640px | 62% 66% | 24px 0px | −1.1 1.1 |

Keep video blend mode **normal**. Above it, a pointer-inert edge-blend layer has these two backgrounds, first listed on top:

```css
background:
  linear-gradient(to right, #0e0e0e 0%, rgba(14,14,14,.6) 2%,
    transparent 8%, transparent 92%, rgba(14,14,14,.6) 98%, #0e0e0e 100%),
  linear-gradient(to bottom, transparent 0%, transparent 82%,
    rgba(14,14,14,.4) 92%, #0e0e0e 100%);
```

The desktop portal sits near the center/right, with the dark forest at the left behind copy. Mobile intentionally favors the dark forest crop; forcing the portal to remain centered would differ from the reference. A wrapper adds gentle positive-Y parallax `min(16px, scrollY * .04)`; disable it with reduced motion. Poster-only mode must use the **same crop, transforms, and edge blend**, not an untransformed background image.

#### Section background compositing

- **Lab:** `filter:saturate(1.58) brightness(1.28) contrast(1.16)` on the raster, `mix-blend-mode:screen` on its wrapper; overlay a top-to-bottom surface/70 → transparent gradient, 350px high. Content sits above both layers.
- **Compute:** Same filter; `mix-blend-mode:lighten`. Keep the left-to-right corridor of RGB bars; do not replace it with dots or add the Lab gradient by default.
- **Inference:** Same filter, normal image blend on black. Above it: horizontal gradient `linear-gradient(90deg,rgba(0,0,0,.46) 0%,rgba(0,0,0,.2) 42%,transparent 78%,rgba(0,0,0,.08) 100%)`; vertical gradient `linear-gradient(180deg,rgba(14,14,14,.04) 0%,transparent 42%,rgba(14,14,14,.18) 100%)`; finally a screen-blended wash `radial-gradient(circle at 72% 38%,rgba(125,255,166,.17),transparent 34%), radial-gradient(circle at 48% 52%,rgba(73,220,255,.1),transparent 30%)`. Copy above all.
- **Customer stories:** Keep the original image crop. Bottom legibility overlay is `linear-gradient(transparent,rgba(0,0,0,.8) 24% 100%)`, in a 165px-high strip. Titles and category chips sit at the bottom, logos at top-left.

### Buttons

Marketing buttons: `28px` high/min-height, Favorit Mono 12px uppercase, `leading-none`, square corners, no shadow, `transition-colors`. Primary buttons use 8px padding (12px text + 16px vertical padding); keep short labels on one line. Secondary buttons use 28px explicit height and 10px horizontal padding. Focus ring is `outline-2 outline-offset-2 outline-white/60`. The mobile menu trigger is a separate 48px control. Component `padding` tokens represent nominal inset; these axis-specific rules resolve shorthand ambiguity.

- **Primary:** Solid `#ffffff` fill, `#000000` text, `8px` horizontal padding. One primary per **CTA group**, not per viewport: header, hero, and several visible feature cells can each have one. `Start Training`, `Get Compute`, `Create Environments`, `Run your first eval`, and `Discover` are white buttons in the reference. Hover: slightly dimmed white.
- **Secondary (glass):** `bg-white/10`, `border-white/16`, `text-white/85`, `backdrop-blur-md`, `10px` padding, optional trailing arrow icon. Hover: `bg-white/16`, `border-white/28`, `text-white`. Used for `Book a Call`, `Login`, and section CTAs.
- **Ghost:** Transparent, `text-white/50`. Hover: `bg-white/16`, `text-white`. Nav-level actions only.
- **Link:** Uppercase Favorit Mono at 12px, white, underline-on-hover with `underline-offset-4` and `decoration-white/60` for plain text links. Do not downgrade the homepage's white CTAs to this variant merely because they navigate.

### Cards & Cells

- Bordered `#202020`, `20px` padding, no radius. Default cards have no shadow; inference service cards are the documented subtle exception. Testimonial cards use min-height 300px; Customer Stories uses 490px-high cards.
- Contents: figure label (`FIG.n`, 10px mono, 25% white) top-left; title in Geist 20px; description in 12–14px at 50% white; CTA bottom-left.
- Hover: background `rgba(255,255,255,0.02)`, description text → 70% white. Editorial/story cards can be whole-card links. Feature panels containing CTAs, copy controls, or demos are not one nested anchor; keep each interaction separately focusable.

### Chips / Tags

- `bg-white/6`, `text-white/50`, Favorit Mono 10–12px uppercase, `4px` padding, square.
- Active: `bg-white/10`, `text-white`.
- Status "available": `#85ed75` solid dot or `rgba(94,233,181,0.15)` fill with `rgba(94,233,181,0.2)` border.

### Inputs

- `36px` tall, `bg-white/4`, `border-[#2a2a2a]`, Geist 14px, `8px` padding, square. Placeholder at 30% white.
- Focus: border → `rgba(255,255,255,0.6)`. Error: border → `#ef4444`, helper text in `#ef4444` at 12px.
- Labels: Favorit Mono 12px uppercase at 50% white, above the field.

### Lists

- Numbered (`1.1`, `1.2`, `1.3`) with the number in 50% white and the label in 100% white, both Geist. Sub-descriptions at 12–14px, 50% white.
- Dividers are 1px `#202020`; no bullets, no icons unless schematic line-icons.

### Code / Terminal

- Geist Mono 12px on `#111111`, text 75% white, `16px` padding, bordered.
- Hero install line: `$` uses `#78e772` and the `18px` glow; `pip install prime` is Geist Mono 14px at white/46, with a faint 24px green text shadow. A white/60 block cursor, .55em wide and 1.05em high, blinks every 1.1s at the end. Preserve case and spacing. Code-copy controls copy text only; they must never execute the command.

### Tooltips

- `bg-white/6`, Favorit Mono 10px (the `label-sm` token), `4px` padding, `4px` radius, soft shadow `0 10px 28px rgba(0,0,0,0.18)`. Suggested utility pattern, no arrow; make available on keyboard focus as well as hover.

### Header / Nav

- Fixed navigation row, 64px; transparent over the hero → tinted/blurred on scroll. Desktop announcement strip is 36px high and appears only at `xl`; it collapses on scroll and the row moves upward. Do not reserve a permanent 100px spacer in the hero.
- Desktop order: original logo; TRAINING 01, INFERENCE 02, COMPUTE 03, RESEARCH 04; DOCS, BLOG, CAREERS plus count, BOOK A CALL; LOGIN and START TRAINING. Product tiles are **124×30px**, gap 4px, white/25 background (white/33 hover), label white/88 and number white/45. Utility links use white/90, not white/50. Login is glass, not ghost.
- Announcement: centered `Introducing Prime Agent ↗`, divider, copyable install command; small mono type, dark translucent background, faint bottom border. Article and copy button must be distinct interaction targets.
- Below 1280px: logo left and square **48×48px** menu trigger right. Open state is a full-width dark sheet **below the 64px row**, `#1b1b1b`, padding 20px, vertically stacked 30px navigation rows with 4px gaps and CTAs below. It slides from the top, not a right-side drawer. Menu icon changes to an X. Provide a named dialog, `aria-expanded`, Escape-to-close, focus containment/restoration, and close on link selection.

### Product figures and embedded demos

The following are distinct assets/components; generic isometric cubes are not acceptable replacements:

- **FIG.1 — RL workflow:** Use the manifest SVG unchanged. Commands surround four nodes on a diamond inside faint overlapping circles; figure stage centered, no card fill behind the diagram.
- **FIG.2 — Evaluations:** Inline SVG **viewBox `0 0 521 402`**, not a named image URL. Shows a dim result table behind a brighter environment card: `primeintellect/logic-env`, version, owner, and a full-width white Evaluate strip. Right-hand rows carry truncated answers and green/red/amber decimal scores. Surrounding rows fade toward the edges. Keep this table composition rather than inventing a generic analytics chart.
- **FIG.3 — Hosted Training:** HTML/SVG reward chart on the left, compact parameter list on the right, separated by a faint vertical dashed divider. Thin jagged **white** curve climbs from 0 toward 1; green readout `0.92` near top-right, subtle grid, 0–1 Y axis. Parameters: max_steps 10,000; rollouts_per_example 19; seq_len 4; batch_size 65536; max_tokens 256; learning_rate 0.00005. Labels gray, values white, dotted leaders. The reference animates the curve/parameter emphasis; a static reproducible curve is a declared fallback.
- **FIG.4 — Inference:** Inline SVG **viewBox `0 0 700 402`**: wireframe hemisphere arcs, sparse white square nodes and connecting paths. Model identifiers/logos run across the top, central item bright with a tiny green status marker and flanking items faded. This is a globe/network figure, not the exploded stack used later.
- **Continual training loop:** Triangular line diagram with COLLECT DATA at top, DEPLOY bottom-left, RETRAIN bottom-right; smaller dotted triangle and MODEL label inside. White arrowheads travel around the loop. Preserve a static triangle with arrows in reduced-motion mode.
- **Environment Hub preview:** Top tabs Explore / My Stars / My Environments, active underline, Featured count and Show All. Dark 3-column miniature card grid with owner/avatar, star count, environment name, clamped description, small tags, update status, and version. Lower content is deliberately clipped/faded. Below it, three code panels show environment development, training configuration, and inference usage with tiny language/file markers and copy buttons.
- **Compute previews:** Single Node / Multi Node tabs above a dense GPU list: NVIDIA mark, hardware name, memory, price, available status and separators. The reserved-capacity example is a bordered floating-within-stage panel (not an actual popover) with GPU count/price, segmented capacity meter, usage rows, and green resell value on `fig-7-bg.png`.

For exact vector fidelity, extract FIG.2/FIG.4's inline `<svg>` from the homepage: locate the Lab cell whose label is `FIG.2`/`FIG.4`, then its figure-stage SVG with the viewBox above. Preserve `defs`, masks, clip paths, text, and all paths; prefix IDs when embedding more than once. There is **no verified `/backgrounds/evaluations.svg` or reward-curve image**; do not invent asset URLs. A maintainable HTML/SVG reconstruction following these recipes gives style fidelity, but requires reference comparison before claiming pixel parity. Never treat sample scores, availability, counts, or dates as live data without a backend.

### Footer and motion

- Footer uses original wordmark, a desktop-only **666×205px canvas** rendering `Be your own Lab.` as very faint dot-matrix type, and a white/glass CTA pair. It responds to the pointer; static dotted text is the reduced-motion/no-canvas fallback, with equivalent accessible text. Do not replace this with a huge solid sans heading.
- Link groups: Platform, Company, Community, Resources, Terms. Group labels are 12px Favorit Mono at white/40; links white, uppercase, 12px, 12px row gaps. Copyright sits below. Careers counts and year are content, not styling tokens.
- Color transitions are typically 150–200ms; CTA chevrons slide horizontally in a clipped 12px box over 300ms; story panels resize over 500ms. Hero title has a faint 9s light wash and a 140px-radius pointer-follow highlight, not a whole-page neon glow. Cursor blink is a step-like on/off animation.
- **Recommended accessibility behavior:** Honor `prefers-reduced-motion`: poster instead of video, no parallax, static hero title/cursor, static chart/loop/canvas, and no animated GIFs (use a generated still). Pause offscreen decorative animation and provide a pause option for persistent motion. These are implementation requirements for an accessible adaptation, not claims that all were verified in the live site.
- Preload the hero poster and essential fonts; lazy-load lower-page media with reserved dimensions. Wait for fonts and images before screenshot comparisons. Keep loading/error states inside the reserved media area; never collapse a missing 402px figure into an empty line.

### Accessibility and fidelity checks

Decorative images use `alt=""`/`aria-hidden`; partner logo links have meaningful accessible names; the team photo has descriptive alt text. Interactive controls require visible focus, pointer and keyboard access, and text labels for icon-only actions. Expand hit areas for compact controls where possible without changing their visual 28px height.

The reference deliberately uses low-contrast decorative figure numbers and kickers. Do not describe that as universally WCAG-compliant. **50% white over `#0e0e0e` is about 5.34:1, not 8:1; 30% is about 2.64:1 and 25% about 2.19:1.** Required body/interactive text must reach 4.5:1 on its actual composited background; essential large text needs 3:1. Raise opacity where necessary and document the accessibility deviation. Contrast tooling may misread translucent component tokens without the underlying surface; validate the rendered composite, including images. The alpha linter also reports tokens used only in prose as unreferenced by components. These advisories must not be silenced by changing the reference palette or fabricating token references; essential low-contrast figure text needs a genuinely higher-contrast treatment in an accessible implementation.

Before claiming faithful reproduction: compare 1440×1000 and 390×844 screenshots, additionally exercise 768/1024/1280px breakpoints, scroll to load all media, open/close the mobile menu with keyboard, and check the hero poster under reduced motion. Verify hero crop, fonts/line breaks, five-column logos, 2×2 Lab grid, original diagrams, image filters, Ramp color, and footer. Freeze motion at the same frame for image diffs; otherwise moving imagery creates false mismatches. Asset URLs and this document alone are not proof that an unbuilt page is pixel-identical.

## Do's and Don'ts

- **Do** build hierarchy with size and white-alpha opacity (100 / 50 / 30 / 25%). **Don't** use font weight above 500.
- **Do** keep every container square. **Don't** add border-radius to cards, buttons, or inputs.
- **Do** use 1px `#202020` hairlines to separate content. **Don't** use drop shadows or gaps to separate cards.
- **Do** reserve signal green for prompts, live status, data readouts, and hero glow. **Don't** color primary buttons green; preserve natural/video colors and the explicit Ramp-branded exception.
- **Do** set marketing labels, buttons, and eyebrows in ABC Favorit Mono, uppercase. **Don't** uppercase commands or model identifiers, or set body copy in the mono face.
- **Do** number product subfeatures and label Lab figures (`01`, `1.1`, `FIG.1`). **Don't** add numbering to every unrelated list or substitute icons for these annotations.
- **Do** keep one white primary button per CTA group. **Don't** remove header/section CTAs to enforce an invented one-per-viewport limit.
- **Do** use `#0e0e0e` as the main ground and the documented subtle surface lifts. **Don't** invent a light theme; keep the black inference banner, cool research media, and bright Ramp testimonial as explicit exceptions.
- **Do** maintain ≥4.5:1 contrast for essential body/interactive text on the actual composite. **Don't** mistake faint reference annotations for accessible body styles or assume transparency is checked correctly against white.
- **Do** use restrained glass for secondary controls and header layers. **Don't** blur whole cards or content sections.
- **Do** preserve each figure's original geometry: workflow, table, chart, globe, stack, or triangle. **Don't** replace every visual with isometric cubes, flat icons, or stock illustration.
- **Do** use the exact hero media and per-section crop/blend recipes. **Don't** replace the forest video with an LED field, apply one blend mode everywhere, or claim generated substitutes are exact assets.
- **Do** load and pin authorized assets/fonts, test mobile and reduced motion, and report substitutions. **Don't** hotlink hashed font URLs as a permanent production dependency or infer reuse permission from a successful download.

## Kiln adaptation — 2026-10-19

The reference above was imported from `design.md` in Amp thread
`T-01a06f2f-682b-7209-ba48-4b416d291c96` at the owner's request. It describes
Prime Intellect, not Kiln's features or affiliations. Keep that reference intact;
this section defines the deliberate adaptations for Kiln's public page and console.

- Apply the neutral palette, square controls, hairlines, regular Geist headings,
  white primary actions, and green prompts/status to both surfaces. Use the bundled,
  open-source Geist Mono for labels instead of ABC Favorit Mono trial fonts.
- `/` is a public, API-independent landing page for developers operating their own
  Linux host. Its job is to explain the product and lead to client installation and
  server enrollment. `/boxes` remains the console; preview stays explicit at
  `/boxes?preview=1`. Do not claim browser login or live VM controls are implemented.
- Use Kiln's real CLI commands and existing setup documentation. Do not copy Prime
  Intellect's logos, customers, testimonials, benchmark numbers, or product claims.
- Retain console information density and mobile navigation. Use white active
  navigation and focus indicators; green denotes signals, not primary actions.
  Raise essential text contrast and use 44px landing-page touch targets rather than
  the reference's compact 28px marketing buttons. No new backend contracts.
- The public hero is an original, static landscape commissioned with Amp Painter:
  a smoked-glass computing cabinet on basalt in a misty conifer valley at dusk,
  with dark negative space for copy. It is illustrative, not a photograph of a real
  installation. No third-party reference imagery is shipped. Static imagery avoids
  autoplay and reduced-motion differences. The title is larger than the reference
  (36–56px) to support Kiln's shorter two-line headline.
- Committed asset: `web/src/assets/kiln-landscape.webp`, 1738×905, SHA-256
  `6569f260e4619dcac50fce94cab83b4b20bfbd6e356e0975ec97d2b9bb3dfba3`.
  The generated PNG was converted to WebP at quality 0.86 using Chromium canvas
  without resizing, recoloring, sharpening, or mirroring. Desktop crop is 60% center;
  mobile is 74% center with a stronger lower legibility overlay. Exact generation
  seed/model are unavailable. The source prompt and generated image are retained in
  this implementation thread, `T-01a0ef91-e214-71d6-80ab-ac7c12ac47c0`.
- Verify both surfaces at desktop and phone widths, plus intermediate breakpoints.
  Exercise navigation, install-command copy (and failure), explicit preview and
  disconnected states, existing dialogs, and automated accessibility checks.
