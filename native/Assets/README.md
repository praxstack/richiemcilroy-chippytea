# macOS app icon

`AppIcon.icns` is the app's Finder and System Settings icon. It uses the same
golden fish and paper palette as the native app and website, with standard and
Retina representations from 16 to 1024 pixels.

The build copies this committed file into `Contents/Resources` before signing;
`CFBundleIconFile` in `native/Info.plist` identifies it. No artwork tools are
needed for normal development or release builds.

To regenerate both the SVG source and ICNS on macOS, install Bun and librsvg
(`brew install librsvg`), then run `bun scripts/generate-app-icon.ts` from the
repository root. Use `bun scripts/generate-app-icon.ts --check` to verify that
the generated files are current. Edit the generator, not its generated files.
