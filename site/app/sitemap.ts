import type { MetadataRoute } from "next";
import { siteUrl } from "@/lib/site";

// One page. The download route is a redirect, not a page.
export default function sitemap(): MetadataRoute.Sitemap {
  return [{ url: siteUrl().href, lastModified: new Date(), changeFrequency: "monthly", priority: 1 }];
}
