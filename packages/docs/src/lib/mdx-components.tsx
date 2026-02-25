import {
  Info,
  Warning,
  Lightbulb,
  Code,
} from "@phosphor-icons/react/dist/ssr";
import remarkGfm from "remark-gfm";
import { cn } from "./utils";
import { Playground } from "@/components/Playground/MdxPlayground";
import { Figure } from "@/components/Figure";
import { BeforeAfter } from "@/components/BeforeAfter";
import { Steps } from "@/components/Steps";

/**
 * Renders a table of API methods with signatures.
 */
interface ApiMethod {
  name: string;
  signature?: string;
  description: string;
}

interface ApiTableProps {
  methods: ApiMethod[];
  title?: string;
}

export function ApiTable({ methods, title }: ApiTableProps) {
  return (
    <div className="my-6 overflow-x-auto">
      {title && (
        <h4 className="text-sm font-semibold text-text-muted mb-2">{title}</h4>
      )}
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-border">
            <th className="text-left py-2 pr-4 font-mono font-medium text-sm">Method</th>
            <th className="text-left py-2 font-mono font-medium text-sm">Description</th>
          </tr>
        </thead>
        <tbody>
          {methods.map((method) => (
            <tr key={method.name} className="border-b border-border/50">
              <td className="py-2 pr-4 align-top">
                <code className="text-accent bg-accent/10 px-1.5 py-0.5 rounded text-xs">
                  {method.signature || method.name}
                </code>
              </td>
              <td className="py-2 text-text-body">{method.description}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * Info/warning/tip callout boxes.
 */
type CalloutType = "info" | "warning" | "tip" | "code";

interface CalloutProps {
  type?: CalloutType;
  title?: string;
  children: React.ReactNode;
}

const calloutStyles: Record<
  CalloutType,
  { bg: string; border: string; icon: typeof Info; iconColor: string }
> = {
  info: {
    bg: "bg-blue-500/10",
    border: "border-blue-500/30",
    icon: Info,
    iconColor: "text-blue-400",
  },
  warning: {
    bg: "bg-yellow-500/10",
    border: "border-yellow-500/30",
    icon: Warning,
    iconColor: "text-yellow-400",
  },
  tip: {
    bg: "bg-green-500/10",
    border: "border-green-500/30",
    icon: Lightbulb,
    iconColor: "text-green-400",
  },
  code: {
    bg: "bg-purple-500/10",
    border: "border-purple-500/30",
    icon: Code,
    iconColor: "text-purple-400",
  },
};

export function Callout({ type = "info", title, children }: CalloutProps) {
  const style = calloutStyles[type];
  const Icon = style.icon;

  return (
    <div
      className={cn("my-6 p-4 rounded-lg border", style.bg, style.border)}
    >
      <div className="flex gap-3">
        <Icon
          size={20}
          weight="fill"
          className={cn("flex-shrink-0 mt-0.5", style.iconColor)}
        />
        <div className="min-w-0">
          {title && <div className="font-semibold mb-1">{title}</div>}
          <div className="text-sm [&>p]:m-0 [&>p:not(:last-child)]:mb-2">
            {children}
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * TypeScript type definition display.
 */
interface TypeDefProps {
  name: string;
  children: React.ReactNode;
}

export function TypeDef({ name, children }: TypeDefProps) {
  return (
    <div className="my-6">
      <div className="flex items-center gap-2 mb-2">
        <span className="text-xs font-medium text-purple-400 bg-purple-500/10 px-2 py-0.5 rounded">
          TYPE
        </span>
        <code className="font-bold text-text">{name}</code>
      </div>
      <div className="bg-surface border border-border rounded-lg p-4 overflow-x-auto">
        <pre className="text-sm">
          <code>{children}</code>
        </pre>
      </div>
    </div>
  );
}

/**
 * Param definition for API docs.
 */
interface ParamProps {
  name: string;
  type: string;
  required?: boolean;
  children: React.ReactNode;
}

export function Param({ name, type, required = true, children }: ParamProps) {
  return (
    <div className="py-3 border-b border-border/50 last:border-0">
      <div className="flex items-center gap-2 mb-1">
        <code className="text-accent font-medium">{name}</code>
        <span className="text-xs text-text-muted">{type}</span>
        {required && <span className="text-xs text-red-400">required</span>}
        {!required && (
          <span className="text-xs text-text-muted">optional</span>
        )}
      </div>
      <div className="text-sm text-text-body">{children}</div>
    </div>
  );
}

/**
 * MDX options with remark-gfm for table/strikethrough/autolink support.
 */
export const mdxOptions = {
  mdxOptions: {
    remarkPlugins: [remarkGfm],
  },
};

/**
 * Returns component map for MDXRemote.
 */
export function getMdxComponents() {
  return {
    Playground,
    Figure,
    BeforeAfter,
    Steps,
    ApiTable,
    Callout,
    TypeDef,
    Param,
    // Override default elements for consistent styling
    h1: (props: React.HTMLAttributes<HTMLHeadingElement>) => (
      <h1 className="text-4xl font-bold font-mono tracking-tight mt-8 mb-4 first:mt-0" {...props} />
    ),
    h2: (props: React.HTMLAttributes<HTMLHeadingElement>) => (
      <h2 className="text-2xl font-bold font-mono tracking-tight mt-8 mb-4 pb-3 border-b border-border" {...props} />
    ),
    h3: (props: React.HTMLAttributes<HTMLHeadingElement>) => (
      <h3 className="text-xl font-semibold font-mono tracking-tight mt-6 mb-3" {...props} />
    ),
    h4: (props: React.HTMLAttributes<HTMLHeadingElement>) => (
      <h4 className="text-lg font-semibold font-mono tracking-tight mt-4 mb-2" {...props} />
    ),
    p: (props: React.HTMLAttributes<HTMLParagraphElement>) => (
      <p className="my-4 leading-relaxed text-text-body" {...props} />
    ),
    ul: (props: React.HTMLAttributes<HTMLUListElement>) => (
      <ul className="my-4 pl-6 list-disc space-y-2 text-text-body" {...props} />
    ),
    ol: (props: React.HTMLAttributes<HTMLOListElement>) => (
      <ol className="my-4 pl-6 list-decimal space-y-2 text-text-body" {...props} />
    ),
    li: (props: React.HTMLAttributes<HTMLLIElement>) => (
      <li className="leading-relaxed" {...props} />
    ),
    code: (props: React.HTMLAttributes<HTMLElement>) => {
      // Check if this is inline code (not inside pre)
      const isInline = typeof props.children === "string";
      if (isInline) {
        return (
          <code
            className="bg-accent/10 text-accent px-1.5 py-0.5 rounded text-sm font-mono"
            {...props}
          />
        );
      }
      return <code {...props} />;
    },
    pre: (props: React.HTMLAttributes<HTMLPreElement>) => (
      <pre
        className="my-4 p-4 bg-surface border border-border rounded-lg overflow-x-auto text-sm"
        {...props}
      />
    ),
    table: (props: React.HTMLAttributes<HTMLTableElement>) => (
      <div className="my-4 overflow-x-auto">
        <table className="w-full text-sm" {...props} />
      </div>
    ),
    thead: (props: React.HTMLAttributes<HTMLTableSectionElement>) => (
      <thead className="border-b border-border" {...props} />
    ),
    th: (props: React.HTMLAttributes<HTMLTableCellElement>) => (
      <th className="text-left py-3 px-4 font-mono font-medium text-sm" {...props} />
    ),
    td: (props: React.HTMLAttributes<HTMLTableCellElement>) => (
      <td className="py-3 px-4 border-b border-border/50 text-text-body" {...props} />
    ),
    a: (props: React.AnchorHTMLAttributes<HTMLAnchorElement>) => (
      <a
        className="text-accent hover:text-accent-hover underline underline-offset-2"
        {...props}
      />
    ),
    blockquote: (props: React.HTMLAttributes<HTMLQuoteElement>) => (
      <blockquote
        className="my-4 pl-4 border-l-2 border-accent/50 text-text-muted italic"
        {...props}
      />
    ),
    hr: (props: React.HTMLAttributes<HTMLHRElement>) => (
      <hr className="my-8 border-border" {...props} />
    ),
    strong: (props: React.HTMLAttributes<HTMLElement>) => (
      <strong className="font-semibold text-text" {...props} />
    ),
  };
}
