import type { MetadataRoute } from "next";
import { siteUrl } from "@/lib/site";

// Everything is crawlable except the download redirect, which only forwards
// to GitHub and would otherwise cost every crawler a release lookup.
export default function robots(): MetadataRoute.Robots {
  return {
    rules: [{ userAgent: "*", allow: "/", disallow: ["/download"] }],
    sitemap: new URL("/sitemap.xml", siteUrl()).href,
  };
}
