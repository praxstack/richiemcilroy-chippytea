# README artwork

The fish, wordmark, chips, and paper use the same seeded drawing primitives as the landing page. They are original chippytea artwork, covered by the repository's [MIT license](../../LICENSE).

The light and dark SVGs are self-contained. They use no scripts, external images, downloaded fonts, tracking pixels, or remote animation service. The animated illustration uses example data, not a screenshot or a benchmark. Its motion stops when the viewer requests reduced motion.

Regenerate from the repository root with Bun:

```sh
bun scripts/generate-readme-art.ts
bun scripts/generate-readme-art.ts --check
```

Edit the generator, not the generated SVGs. Commit both together. The README selects its theme with GitHub's supported [picture element](https://docs.github.com/en/get-started/writing-on-github/getting-started-with-writing-and-formatting-on-github/quickstart-for-writing-on-github).
