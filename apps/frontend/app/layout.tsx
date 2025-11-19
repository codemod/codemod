import { mediaStyles } from "@/components/global/Media";
import globalFontsVariables from "@/fonts";
import dynamicFavicon from "@/headScripts/dynamic_favicon";
import themeScript from "@/headScripts/theme";
import { GoogleAnalytics, GoogleTagManager } from "@next/third-parties/google";
import { Analytics } from "@vercel/analytics/react";
import { cx } from "cva";
import Script from "next/script";
import { headers } from "next/headers";

// DO NOT import global CSS here - it will affect Payload routes
// Global CSS is imported in (website)/layout.tsx instead
export default async function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  const headersList = await headers();
  const isPayloadRoute = headersList.get("x-is-payload-route") === "true";

  // For Payload routes, let Payload's RootLayout handle everything - no interference
  // This prevents our global CSS and HTML structure from affecting Payload
  if (isPayloadRoute) {
    return <>{children}</>;
  }

  const gtmId = process.env.NEXT_PUBLIC_GTM_ID;
  const gaId = process.env.NEXT_PUBLIC_GA_ID;

  return (
    <html lang="en" className={cx(globalFontsVariables, "scroll-smooth light")}>
      <GoogleTagManager gtmId={gtmId as string} />
      <GoogleAnalytics gaId={gaId as string} />
      <head>
        <script dangerouslySetInnerHTML={{ __html: dynamicFavicon }} />
        <script dangerouslySetInnerHTML={{ __html: themeScript }} />
        <style
          key="fresnel-css"
          dangerouslySetInnerHTML={{ __html: mediaStyles }}
          type="text/css"
        />
        <Script
          async
          src="https://cdn.promotekit.com/promotekit.js"
          data-promotekit="34692db8-7e9a-4d0a-a7ba-b84b84ffc70a"
        />
        <Script>
          {`document.addEventListener("DOMContentLoaded", function () {
            setTimeout(function () {
                document.querySelectorAll('a[href^="https://buy.stripe.com/"]').forEach(function (link) {
                    const oldBuyUrl = link.getAttribute("href");
                    const referralId = window.promotekit_referral;
                    if (!oldBuyUrl.includes("client_reference_id")) {
                        const newBuyUrl = oldBuyUrl + "?client_reference_id=" + referralId;
                        link.setAttribute("href", newBuyUrl);
                    }
                });

                document.querySelectorAll("[pricing-table-id]").forEach(function (element) {
                    element.setAttribute("client-reference-id", window.promotekit_referral);
                });

                document.querySelectorAll("[buy-button-id]").forEach(function (element) {
                    element.setAttribute("client-reference-id", window.promotekit_referral);
                });
            }, 1500);
        });`}
        </Script>
      </head>
      <body>
        {/* Google Tag Manager (noscript) */}
        <noscript>
          <iframe
            src={`https://www.googletagmanager.com/ns.html?id=${gtmId}`}
            height="0"
            width="0"
            style={{ display: "none", visibility: "hidden" }}
          />
        </noscript>
        {/* End Google Tag Manager (noscript) */}
        {children}
        <Analytics />
      </body>
    </html>
  );
}
