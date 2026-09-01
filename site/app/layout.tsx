import type { Metadata, Viewport } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "chippytea: tidy your disk, earn your tea",
  description:
    "Review old build files and downloads, make space, and earn chips for credited storage. A native Mac menu-bar app built with SwiftUI and Rust. Free, MIT licensed.",
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
