<!--
  Adapted from the MIT-licensed `ease-design` skill (`knowledge/design-tells.md` and
  `knowledge/taste-rubric.md`), condensed into DESIGN:OS Studio's own wording (E15-15 §3.3).
-->

## Design Quality & Anti-Generic Rules

Enforce these qualitative standards alongside automated linting:

- **Visual hierarchy**: Establish one primary focal point per screen. Maintain at least a 3:1 size ratio between display headlines and body copy. Never give primary, secondary, and tertiary actions equal visual weight.
- **Layout rhythm**: Use at most 3 distinct gap values per screen. Padding must increase with container hierarchy (`page >= section >= card >= control`). Never nest cards inside cards, and never mix left-aligned and centred section headers in the same vertical flow.
- **Colour discipline**: Use one primary accent per screen. Reserve status colours (red, amber, green) strictly for semantic status. Let neutral surface elevation carry hierarchy rather than saturated backgrounds; in dark mode, express depth via surface luminance and subtle borders rather than inverted light-mode shadows.
- **Typography**: Use at most 2 font families and 4 font sizes per screen. Keep body copy line length between 45 and 75 characters, and use tabular numerals (`tabular-nums`) for prices, metrics, timestamps, and data tables.
- **Content realism**: Never use `"Lorem ipsum"`, `"Card 1 / Card 2 / Card 3"`, `"John Doe"`, `"$99.99"`, `"123456"`, or identical repeated rows. Populate every screen with domain-specific labels, realistic uneven numbers, and varied states (active, pending, empty, long name).
- **Anti-generic tells**: Avoid gratuitous purple-to-blue gradients, glowing neon borders on enterprise workspaces, 3-equal-column icon-title-blurb grids when a domain-specific layout fits the content, and decorative charts without axes or units.
