/**
 * 16×16 schematic symbol icons, shared by the circuit tool row and anywhere
 * else that needs a glyph for a placeable component. Extracted from the old
 * bottom ElectronicsToolbar.
 */

export function SymbolIcon({ id }: { id: string }) {
  const s = 16; // viewBox size
  const common = {
    width: s,
    height: s,
    viewBox: `0 0 ${s} ${s}`,
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.5,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
  };

  switch (id) {
    case "resistor": // IEC rectangle
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="4" y2="8" />
          <rect x="4" y="5" width="8" height="6" rx="0.5" />
          <line x1="12" y1="8" x2="15" y2="8" />
        </svg>
      );
    case "capacitor": // two parallel lines
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="6" y2="8" />
          <line x1="6" y1="3" x2="6" y2="13" />
          <line x1="10" y1="3" x2="10" y2="13" />
          <line x1="10" y1="8" x2="15" y2="8" />
        </svg>
      );
    case "led": // diode triangle + arrows
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="5" y2="8" />
          <polygon points="5,4 5,12 11,8" fill="currentColor" opacity="0.3" stroke="currentColor" />
          <line x1="11" y1="4" x2="11" y2="12" />
          <line x1="11" y1="8" x2="15" y2="8" />
          <line x1="9" y1="2" x2="12" y2="0" />
          <line x1="11" y1="3" x2="14" y2="1" />
        </svg>
      );
    case "diode": // triangle + bar
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="5" y2="8" />
          <polygon points="5,4 5,12 11,8" fill="currentColor" opacity="0.3" stroke="currentColor" />
          <line x1="11" y1="4" x2="11" y2="12" />
          <line x1="11" y1="8" x2="15" y2="8" />
        </svg>
      );
    case "npn": // transistor
      return (
        <svg {...common}>
          <line x1="1" y1="8" x2="5" y2="8" />
          <line x1="5" y1="4" x2="5" y2="12" />
          <line x1="5" y1="6" x2="12" y2="2" />
          <line x1="5" y1="10" x2="12" y2="14" />
          <circle cx="8" cy="8" r="6" strokeWidth="1" opacity="0.4" />
        </svg>
      );
    case "ic8": // chip rectangle with pins
      return (
        <svg {...common}>
          <rect x="4" y="2" width="8" height="12" rx="0.5" />
          <line x1="1" y1="5" x2="4" y2="5" />
          <line x1="1" y1="8" x2="4" y2="8" />
          <line x1="1" y1="11" x2="4" y2="11" />
          <line x1="12" y1="5" x2="15" y2="5" />
          <line x1="12" y1="8" x2="15" y2="8" />
          <line x1="12" y1="11" x2="15" y2="11" />
          <circle cx="6" cy="4" r="0.8" fill="currentColor" stroke="none" />
        </svg>
      );
    case "vcc": // power up arrow
      return (
        <svg {...common}>
          <line x1="8" y1="14" x2="8" y2="5" />
          <polyline points="4,5 8,1 12,5" />
        </svg>
      );
    case "gnd": // ground symbol
      return (
        <svg {...common}>
          <line x1="8" y1="2" x2="8" y2="7" />
          <line x1="3" y1="7" x2="13" y2="7" />
          <line x1="5" y1="10" x2="11" y2="10" />
          <line x1="7" y1="13" x2="9" y2="13" />
        </svg>
      );
    default:
      return <span className="text-xs font-bold">{id[0]?.toUpperCase()}</span>;
  }
}
