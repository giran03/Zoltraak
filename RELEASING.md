# Releasing Zoltraak

## Release notes

`CHANGELOG.json` is the source for the app dialog, guide changelog, and updater notes. Document each meaningful change in the current development version with a unique `id`, a `category`, a user-facing `brief`, nonempty `details`, and exact repository-relative `files`. Preserve earlier release entries and do not mark a version published without verifying publication.

Edit `scripts/templates/guide.html` and `scripts/changelog-view.js` for guide changes. `node scripts/changelog.mjs sync` regenerates the bundled app and extension guides and legal pages. After edits and tests, run `node scripts/changelog.mjs checkpoint`, then `node scripts/changelog.mjs check`; the desktop build enforces this audit. The version manager asks before transferring unpublished development notes and updates the package and Cargo lockfiles.

## Publishing

Desktop releases currently support Windows only.

Signed releases use the configured Zoltraak updater endpoint. Publish the installer, its signature, and `latest.json` together under the matching version tag. Changing the development version alone does not publish a release.

## Motion showcase

The **Showcase and guide to GitHub Pages** Actions workflow runs on pushes to `main`, or manually. It rebuilds `docs/showcase.html` from tracked sources and publishes it as the site homepage alongside `guide.html` and `legal.html`. Run `npm run showcase:pages --prefix app` to prepare the same site locally in `scratch/showcase-pages/`; `docs/` remains ignored.

Deployment is skipped while the repository is private. Make the repository public and enable **Settings → Pages → Source: GitHub Actions**, then run the workflow. The site URL is https://giran03.github.io/Zoltraak/.

The README showreel is a compressed copy of the local `docs/zoltraak-showreel.gif`, tracked at `.github/assets/zoltraak-showreel.gif` (880 px, 12 fps) so GitHub can render it.
