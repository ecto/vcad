import { generateOgImage, SIZE } from "@/lib/og";

export const size = SIZE;
export const contentType = "image/png";

export default function Image() {
  return generateOgImage({ title: "Gallery", breadcrumb: "vcad" });
}
