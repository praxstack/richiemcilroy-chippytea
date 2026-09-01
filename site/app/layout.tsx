import type { Metadata, Viewport } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Chippytea: Free up space on your Mac",
  description:
    "Chippytea is an ultra-performant, native macOS app that clears space on your Mac. Free and open source. No account or telemetry.",
};

export const viewport: Viewport = {
  themeColor: "#FAF5EA",
  colorScheme: "light",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en-GB">
      <body>{children}</body>
    </html>
  );
}
