# JetBrains Icons

Source: [zed-jetbrains-icons](https://github.com/ziishaned/zed-jetbrains-icons) by Zeeshan Ahmad,
a Zed editor extension packaging JetBrains's own icon set. Copied wholesale (both `dark` and
`light` variants, plus the shared `common` set and the `icon_themes/jetbrains-icons.json`
manifest) for reference and provenance — this directory is not read by AgentMesh at runtime.

AgentMesh inlines two glyphs from the `dark` variant directly into `index.html` and
`renderer.js` (see the attribution comments there): the folder icon (`icons/dark/folder.svg`)
and the chevron (`icons/dark/chevron_right.svg`), used for the app's own folder-picker and
collapsible-group affordances. Nothing else in this set is currently used.

Each SVG carries its own copyright header:

> Copyright 2000-2022 JetBrains s.r.o. and contributors. Use of this source code is governed by
> the Apache 2.0 license.

The full license text is in `LICENSE-Apache-2.0.txt` alongside this file.
