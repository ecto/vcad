import Link from "next/link";

export function Footer() {
  return (
    <footer className="border-t border-border">
      <div className="max-w-[720px] mx-auto px-8 py-8">
        <div className="flex flex-col sm:flex-row justify-between items-center gap-4 text-sm text-text-muted">
          <span>mit license</span>
          <div className="flex gap-6">
            <a
              href="https://github.com/ecto/vcad"
              className="hover:text-text transition-colors"
              target="_blank"
              rel="noopener noreferrer"
            >
              github
            </a>
            <a
              href="https://vcad.io"
              className="hover:text-text transition-colors"
              target="_blank"
              rel="noopener noreferrer"
            >
              vcad.io
            </a>
            <a
              href="https://crates.io/crates/vcad"
              className="hover:text-text transition-colors"
              target="_blank"
              rel="noopener noreferrer"
            >
              crates.io
            </a>
            <a
              href="https://www.npmjs.com/search?q=%40vcad"
              className="hover:text-text transition-colors"
              target="_blank"
              rel="noopener noreferrer"
            >
              npm
            </a>
          </div>
        </div>
      </div>
    </footer>
  );
}
