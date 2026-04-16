import { useEffect, useState } from "react";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { User } from "@phosphor-icons/react/dist/ssr/User";
import {
  getProfileByUsername,
  listPublicDocuments,
  type Profile,
  type PublicDocumentMeta,
} from "@vcad/auth";
import { cn } from "@/lib/utils";

interface ProfilePageProps {
  username: string;
}

/**
 * Public profile page rendered at /@username. Lists all public documents
 * for the user. Clicking a doc navigates to /@username/slug.
 */
export function ProfilePage({ username }: ProfilePageProps) {
  const [profile, setProfile] = useState<Profile | null>(null);
  const [docs, setDocs] = useState<PublicDocumentMeta[]>([]);
  const [loading, setLoading] = useState(true);
  const [notFound, setNotFound] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setNotFound(false);

    (async () => {
      try {
        const p = await getProfileByUsername(username);
        if (cancelled) return;
        if (!p) {
          setNotFound(true);
          setLoading(false);
          return;
        }
        setProfile(p);

        const d = await listPublicDocuments(username);
        if (cancelled) return;
        setDocs(d);
      } catch (err) {
        console.error("[ProfilePage] failed to load:", err);
        if (!cancelled) setNotFound(true);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [username]);

  if (loading) {
    return (
      <div className="flex h-screen items-center justify-center bg-bg text-text-muted text-sm">
        Loading @{username}…
      </div>
    );
  }

  if (notFound) {
    return (
      <div className="flex h-screen flex-col items-center justify-center bg-bg gap-3">
        <span className="text-3xl text-text-muted/30">@{username}</span>
        <span className="text-sm text-text-muted">Profile not found</span>
        <a
          href="/"
          className="mt-4 text-xs text-brand hover:underline"
        >
          ← Back to vcad
        </a>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-bg">
      {/* Profile header */}
      <header className="border-b border-border/40 bg-surface">
        <div className="mx-auto max-w-3xl px-6 py-8">
          <div className="flex items-center gap-4">
            {profile?.avatar_url ? (
              <img
                src={profile.avatar_url}
                alt=""
                className="w-14 h-14 rounded-full object-cover"
                referrerPolicy="no-referrer"
              />
            ) : (
              <div className="w-14 h-14 rounded-full bg-brand/15 flex items-center justify-center">
                <User size={24} className="text-brand" />
              </div>
            )}
            <div>
              <h1 className="text-lg font-semibold text-text">
                {profile?.display_name || `@${username}`}
              </h1>
              {profile?.display_name && (
                <p className="text-sm text-text-muted">@{username}</p>
              )}
              {profile?.bio && (
                <p className="text-xs text-text-muted mt-1 max-w-md">
                  {profile.bio}
                </p>
              )}
            </div>
          </div>
        </div>
      </header>

      {/* Documents grid */}
      <main className="mx-auto max-w-3xl px-6 py-8">
        <h2 className="text-xs font-medium text-text-muted uppercase tracking-wider mb-4">
          Public documents ({docs.length})
        </h2>

        {docs.length === 0 ? (
          <p className="text-sm text-text-muted/60 py-8 text-center">
            No public documents yet.
          </p>
        ) : (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {docs.map((doc) => (
              <a
                key={doc.id}
                href={`/@${username}/${doc.slug}`}
                className={cn(
                  "group flex flex-col gap-2 p-4",
                  "border border-border/60 bg-surface",
                  "hover:border-brand/40 hover:bg-hover transition-colors",
                )}
              >
                <div className="flex items-center gap-2">
                  <Cube
                    size={14}
                    className="text-text-muted/50 group-hover:text-brand transition-colors"
                  />
                  <span className="text-sm font-medium text-text truncate">
                    {doc.name}
                  </span>
                </div>
                <div className="flex items-center gap-2 text-[10px] text-text-muted">
                  <span className="font-mono">{doc.slug}</span>
                  <span>·</span>
                  <span>
                    {doc.published_at
                      ? new Date(doc.published_at).toLocaleDateString()
                      : new Date(doc.updated_at).toLocaleDateString()}
                  </span>
                </div>
              </a>
            ))}
          </div>
        )}
      </main>

      {/* Footer */}
      <footer className="border-t border-border/40 py-6 text-center">
        <a href="/" className="text-xs text-text-muted hover:text-brand transition-colors">
          vcad.io
        </a>
      </footer>
    </div>
  );
}
