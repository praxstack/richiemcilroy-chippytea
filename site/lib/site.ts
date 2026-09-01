// The facts every page, metadata file and generated image repeats.

export const site = {
  name: "chippytea",
  title: "chippytea: Free up space on your Mac",
  headline: "Free up space on your Mac.",
  description:
    "chippytea is an ultra-performant, native macOS app that clears space on your Mac. Built with SwiftUI and Rust, it finds old build folders, dependencies and installers you no longer need.",
  github: "https://github.com/richiemcilroy/chippytea",
  author: { name: "Richie McIlroy", url: "https://github.com/richiemcilroy" },
  locale: "en_GB",
  requirements: "Apple Silicon, macOS 14 or later",
} as const;

/// The canonical origin. Set NEXT_PUBLIC_SITE_URL for a custom domain; on
/// Vercel the project's production domain is used; locally it is the dev server.
export function siteUrl(): URL {
  const explicit = process.env.NEXT_PUBLIC_SITE_URL;
  if (explicit) return new URL(explicit);
  const vercel = process.env.VERCEL_PROJECT_PRODUCTION_URL;
  if (vercel) return new URL(`https://${vercel}`);
  return new URL("http://localhost:3000");
}
