import type { Metadata } from "next";
import Link from "next/link";
import { ArrowRight } from "@phosphor-icons/react/dist/ssr";

export const metadata: Metadata = {
  title: "Comparisons",
  description: "See how vcad compares to other CAD tools",
};

const comparisons = [
  {
    slug: "onshape",
    title: "vcad vs Onshape",
    description: "Cloud-native CAD with real-time collaboration vs free open-source local-first CAD.",
  },
  {
    slug: "fusion360",
    title: "vcad vs Fusion 360",
    description: "Autodesk's integrated CAD/CAM/CAE suite vs lightweight open-source parametric CAD.",
  },
  {
    slug: "openscad",
    title: "vcad vs OpenSCAD",
    description: "Programmer's solid 3D CAD modeler vs modern BRep-based parametric CAD.",
  },
  {
    slug: "freecad",
    title: "vcad vs FreeCAD",
    description: "Open-source general-purpose parametric 3D modeler vs web-first CAD with BRep kernel.",
  },
  {
    slug: "cadquery",
    title: "vcad vs CadQuery",
    description: "Python-based parametric CAD scripting vs Rust/TypeScript CAD with web app and AI.",
  },
];

export default function ComparisonsPage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <h1 className="text-4xl font-bold mb-4">Comparisons</h1>
      <p className="text-text-body mb-12 max-w-2xl">
        Honest comparisons between vcad and other CAD tools.
        We'll tell you when the other tool is the better choice.
      </p>

      <div className="space-y-4">
        {comparisons.map((comp) => (
          <Link
            key={comp.slug}
            href={`/vs/${comp.slug}`}
            className="flex items-center gap-4 p-5 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all group"
          >
            <div className="flex-1 min-w-0">
              <h2 className="text-lg font-bold group-hover:text-accent transition-colors">
                {comp.title}
              </h2>
              <p className="text-text-muted mt-1">{comp.description}</p>
            </div>
            <ArrowRight
              size={20}
              className="text-text-muted group-hover:text-accent group-hover:translate-x-1 transition-all flex-shrink-0"
            />
          </Link>
        ))}
      </div>
    </div>
  );
}
