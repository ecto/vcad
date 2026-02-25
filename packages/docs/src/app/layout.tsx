import type { Metadata } from "next";
import { Inter } from "next/font/google";

const inter = Inter({ subsets: ["latin"], variable: "--font-inter" });
import Script from "next/script";
import { Analytics } from "@vercel/analytics/react";
import { SpeedInsights } from "@vercel/speed-insights/next";
import "./globals.css";
import { Navigation } from "@/components/Navigation";
import { ThemeProvider } from "@/components/ThemeProvider";
import { SearchProvider } from "@/components/Search/SearchProvider";

export const metadata: Metadata = {
  metadataBase: new URL("https://docs.vcad.io"),
  title: {
    default: "vcad",
    template: "%s | vcad",
  },
  description: "Open-source CAD for makers. Web app, Rust library, CLI, and AI tools.",
  keywords: ["CAD", "Rust", "3D modeling", "parametric", "STL", "GLTF", "STEP", "BRep", "MCP"],
  authors: [{ name: "vcad" }],
  openGraph: {
    type: "website",
    locale: "en_US",
    url: "https://docs.vcad.io",
    siteName: "vcad",
    title: "vcad",
    description: "Open-source CAD for makers",
  },
  twitter: {
    card: "summary_large_image",
    title: "vcad",
    description: "Open-source CAD for makers",
  },
  icons: {
    icon: "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><text y='.9em' font-size='90'>&#x25E6;</text></svg>",
  },
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" className={inter.variable} suppressHydrationWarning>
      <body className="min-h-screen">
        <ThemeProvider>
          <SearchProvider>
            <div className="flex min-h-screen">
              <Navigation />
              <main className="flex-1 overflow-auto">
                {children}
              </main>
            </div>
          </SearchProvider>
        </ThemeProvider>
        <Analytics />
        <SpeedInsights />
        {/* PostHog Analytics */}
        <Script id="posthog" strategy="afterInteractive">
          {`
            !function(t,e){var o,n,p,r;e.__SV||(window.posthog=e,e._i=[],e.init=function(i,s,a){function g(t,e){var o=e.split(".");2==o.length&&(t=t[o[0]],e=o[1]),t[e]=function(){t.push([e].concat(Array.prototype.slice.call(arguments,0)))}}(p=t.createElement("script")).type="text/javascript",p.async=!0,p.src=s.api_host.replace(".i.posthog.com","-assets.i.posthog.com")+"/static/array.js",(r=t.getElementsByTagName("script")[0]).parentNode.insertBefore(p,r);var u=e;for(void 0!==a?u=e[a]=[]:a="posthog",u.people=u.people||[],u.toString=function(t){var e="posthog";return"posthog"!==a&&(e+="."+a),t||(e+=" (stub)"),e},u.people.toString=function(){return u.toString(1)+".people (stub)"},o="init capture register register_once register_for_session unregister unregister_for_session getFeatureFlag getFeatureFlagPayload isFeatureEnabled reloadFeatureFlags updateEarlyAccessFeatureEnrollment getEarlyAccessFeatures on onFeatureFlags onSessionId getSurveys getActiveMatchingSurveys renderSurvey canRenderSurvey getNextSurveyStep identify setPersonProperties group resetGroups setPersonPropertiesForFlags resetPersonPropertiesForFlags setGroupPropertiesForFlags resetGroupPropertiesForFlags reset get_distinct_id getGroups get_session_id get_session_replay_url alias set_config startSessionRecording stopSessionRecording sessionRecordingStarted captureException loadToolbar get_property getSessionProperty createPersonProfile opt_in_capturing opt_out_capturing has_opted_in_capturing has_opted_out_capturing clear_opt_in_out_capturing debug getPageViewId".split(" "),n=0;n<o.length;n++)g(u,o[n]);e._i.push([i,s,a])},e.__SV=1)}(document,window.posthog||[]);
            posthog.init('phc_7E1znaGCCuijqjELWcAfSKB4zjIzHcVFTB30mKJIeWW', {api_host: 'https://us.i.posthog.com', person_profiles: 'identified_only'});
          `}
        </Script>
      </body>
    </html>
  );
}
