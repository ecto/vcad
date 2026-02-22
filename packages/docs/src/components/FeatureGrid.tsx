import {
  Browser,
  Cube,
  Export,
  Terminal,
  Robot,
  LockOpen,
} from "@phosphor-icons/react/dist/ssr";

const features = [
  {
    icon: Browser,
    title: "Web App",
    description: "full cad in your browser at vcad.io",
  },
  {
    icon: Cube,
    title: "BRep Kernel",
    description: "custom half-edge kernel in rust. sketching, booleans, fillets, sweeps.",
  },
  {
    icon: Export,
    title: "Export",
    description: "stl, gltf, step, dxf, usd. one model, every format.",
  },
  {
    icon: Terminal,
    title: "CLI",
    description: "create and export models from the terminal",
  },
  {
    icon: Robot,
    title: "MCP",
    description: "ai agents can create, inspect, and export models",
  },
  {
    icon: LockOpen,
    title: "Open Source",
    description: "mit licensed. run locally, own your files.",
  },
];

export function FeatureGrid() {
  return (
    <dl className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 lg:gap-8">
      {features.map((feature) => (
        <div key={feature.title} className="space-y-2">
          <dt className="flex items-center gap-2 font-bold text-text">
            <feature.icon size={18} weight="regular" className="text-accent" />
            {feature.title.toLowerCase()}
          </dt>
          <dd className="text-sm text-text-muted">
            {feature.description}
          </dd>
        </div>
      ))}
    </dl>
  );
}
