import type { Metadata, Viewport } from "next";
import { Analytics } from "@vercel/analytics/next";
import { site, siteUrl } from "@/lib/site";
import "./globals.css";

const base = siteUrl();

export const metadata: Metadata = {
  metadataBase: base,
  title: { default: site.title, template: `%s · ${site.name}` },
  description: site.description,
  applicationName: site.name,
  keywords: [
    "free up space on Mac",
    "clear disk space macOS",
    "Mac storage cleaner",
    "delete old node_modules",
    "clean build folders",
    "remove old installers",
    "open source Mac app",
    "Apple Silicon",
  ],
  authors: [site.author],
  creator: site.author.name,
  publisher: site.author.name,
  category: "utilities",
  alternates: { canonical: "/" },
  openGraph: {
    type: "website",
    url: "/",
    siteName: site.name,
    locale: site.locale,
    title: site.title,
    description: site.description,
  },
  twitter: {
    card: "summary_large_image",
    title: site.title,
    description: site.description,
  },
  robots: {
    index: true,
    follow: true,
    googleBot: {
      index: true,
      follow: true,
      "max-image-preview": "large",
      "max-snippet": -1,
      "max-video-preview": -1,
    },
  },
};

export const viewport: Viewport = {
  themeColor: "#FAF5EA",
  colorScheme: "light",
};

// What the app is, for search engines: free, open source, macOS, with its code.
const structuredData = {
  "@context": "https://schema.org",
  "@graph": [
    {
      "@type": "SoftwareApplication",
      "@id": `${base.href}#app`,
      name: site.name,
      url: base.href,
      image: new URL("/opengraph-image.png", base).href,
      description: site.description,
      applicationCategory: "UtilitiesApplication",
      operatingSystem: "macOS 14 or later",
      softwareRequirements: site.requirements,
      isAccessibleForFree: true,
      offers: { "@type": "Offer", price: "0", priceCurrency: "GBP" },
      license: `${site.github}/blob/main/LICENSE`,
      downloadUrl: new URL("/download", base).href,
      sameAs: [site.github],
      author: { "@type": "Person", name: site.author.name, url: site.author.url },
    },
    {
      "@type": "SoftwareSourceCode",
      "@id": `${base.href}#source`,
      name: `${site.name} source code`,
      codeRepository: site.github,
      programmingLanguage: ["Swift", "Rust"],
      runtimePlatform: "macOS",
      license: `${site.github}/blob/main/LICENSE`,
      targetProduct: { "@id": `${base.href}#app` },
    },
  ],
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en-GB">
      <body>
        {children}
        <Analytics />
        <script
          type="application/ld+json"
          dangerouslySetInnerHTML={{ __html: JSON.stringify(structuredData).replace(/</g, "\\u003c") }}
        />
      </body>
    </html>
  );
}
