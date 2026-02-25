import { ImageResponse } from "next/og";
import { readFile } from "fs/promises";
import { join } from "path";

const SIZE = { width: 1200, height: 630 };

// Module-scope font cache
let fontRegular: ArrayBuffer | null = null;
let fontBold: ArrayBuffer | null = null;

async function loadFonts() {
  if (!fontRegular) {
    fontRegular = (
      await readFile(join(process.cwd(), "public/fonts/BerkeleyMono-Regular.otf"))
    ).buffer as ArrayBuffer;
  }
  if (!fontBold) {
    fontBold = (
      await readFile(join(process.cwd(), "public/fonts/BerkeleyMono-Bold.otf"))
    ).buffer as ArrayBuffer;
  }
  return { fontRegular, fontBold };
}

function titleFontSize(title: string): number {
  if (title.length > 60) return 48;
  if (title.length > 35) return 56;
  return 72;
}

export interface OgImageOptions {
  title: string;
  breadcrumb?: string;
  hero?: boolean;
}

export async function generateOgImage({ title, breadcrumb, hero }: OgImageOptions) {
  const fonts = await loadFonts();

  if (hero) {
    return new ImageResponse(
      (
        <div
          style={{
            width: "100%",
            height: "100%",
            display: "flex",
            flexDirection: "column",
            justifyContent: "center",
            padding: "80px",
            backgroundColor: "#09090b",
            fontFamily: "BerkeleyMono",
          }}
        >
          <div style={{ display: "flex", alignItems: "baseline" }}>
            <span
              style={{
                fontSize: 96,
                fontWeight: 700,
                color: "#fafafa",
                letterSpacing: "-0.02em",
              }}
            >
              vcad
            </span>
            <span
              style={{
                fontSize: 96,
                fontWeight: 700,
                color: "#F92672",
              }}
            >
              .
            </span>
          </div>
          <span
            style={{
              fontSize: 32,
              color: "#71717a",
              marginTop: 16,
            }}
          >
            open-source cad for makers
          </span>
          {/* Accent line */}
          <div
            style={{
              position: "absolute",
              bottom: 80,
              left: 80,
              width: 64,
              height: 4,
              backgroundColor: "#F92672",
            }}
          />
          <span
            style={{
              position: "absolute",
              bottom: 80,
              right: 80,
              fontSize: 20,
              color: "#52525b",
            }}
          >
            docs.vcad.io
          </span>
        </div>
      ),
      {
        ...SIZE,
        fonts: [
          { name: "BerkeleyMono", data: fonts.fontRegular, weight: 400 },
          { name: "BerkeleyMono", data: fonts.fontBold, weight: 700 },
        ],
      },
    );
  }

  const fontSize = titleFontSize(title);

  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
          justifyContent: "center",
          padding: "80px",
          backgroundColor: "#09090b",
          fontFamily: "BerkeleyMono",
        }}
      >
        {/* Breadcrumb */}
        {breadcrumb && (
          <span
            style={{
              position: "absolute",
              top: 80,
              left: 80,
              fontSize: 20,
              color: "#52525b",
            }}
          >
            {breadcrumb}
          </span>
        )}
        {/* Title */}
        <span
          style={{
            fontSize,
            fontWeight: 700,
            color: "#fafafa",
            letterSpacing: "-0.02em",
            lineHeight: 1.15,
            maxWidth: "90%",
          }}
        >
          {title}
        </span>
        {/* Accent line */}
        <div
          style={{
            position: "absolute",
            bottom: 80,
            left: 80,
            width: 64,
            height: 4,
            backgroundColor: "#F92672",
          }}
        />
        <span
          style={{
            position: "absolute",
            bottom: 80,
            right: 80,
            fontSize: 20,
            color: "#52525b",
          }}
        >
          docs.vcad.io
        </span>
      </div>
    ),
    {
      ...SIZE,
      fonts: [
        { name: "BerkeleyMono", data: fonts.fontRegular, weight: 400 },
        { name: "BerkeleyMono", data: fonts.fontBold, weight: 700 },
      ],
    },
  );
}

export { SIZE };
