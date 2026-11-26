import Markdown from 'react-markdown';
import type { Components, UrlTransform } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { PlatformBridge } from '../platform/bridge';

export interface SafeMarkdownProps {
  content: string;
  className?: string;
  bridge?: PlatformBridge;
}

/**
 * Validates that a URL is an absolute HTTP or HTTPS URL.
 * Rejects `javascript:`, `data:`, `vbscript:`, protocol-relative, and relative URLs.
 */
export function sanitizeHttpUrl(rawUrl: string | null | undefined): string | null {
  if (!rawUrl) {
    return null;
  }
  const trimmed = rawUrl.trim();
  if (!trimmed) {
    return null;
  }
  try {
    const parsed = new URL(trimmed);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      return null;
    }
    return parsed.href;
  } catch {
    return null;
  }
}

const safeUrlTransform: UrlTransform = (url) => {
  return sanitizeHttpUrl(url) ?? '';
};

function buildComponents(bridge?: PlatformBridge): Components {
  return {
    h1: ({ children }) => (
      <h1 className="mb-3 mt-5 border-b border-slate-800 pb-2 text-xl font-bold tracking-tight text-white first:mt-0">
        {children}
      </h1>
    ),
    h2: ({ children }) => (
      <h2 className="mb-2.5 mt-5 border-b border-slate-800/70 pb-1.5 text-lg font-semibold tracking-tight text-slate-100 first:mt-0">
        {children}
      </h2>
    ),
    h3: ({ children }) => (
      <h3 className="mb-2 mt-4 text-base font-semibold text-slate-100 first:mt-0">
        {children}
      </h3>
    ),
    h4: ({ children }) => (
      <h4 className="mb-1.5 mt-3 text-sm font-semibold text-slate-200 first:mt-0">
        {children}
      </h4>
    ),
    p: ({ children }) => (
      <p className="mb-3 text-sm leading-relaxed text-slate-300 last:mb-0">{children}</p>
    ),
    ul: ({ children }) => (
      <ul className="mb-3 list-disc space-y-1.5 pl-5 text-sm text-slate-300 last:mb-0">
        {children}
      </ul>
    ),
    ol: ({ children }) => (
      <ol className="mb-3 list-decimal space-y-1.5 pl-5 text-sm text-slate-300 last:mb-0">
        {children}
      </ol>
    ),
    li: ({ children }) => <li className="leading-relaxed text-slate-300">{children}</li>,
    blockquote: ({ children }) => (
      <blockquote className="my-3 border-l-2 border-indigo-500/70 bg-slate-900/60 py-1.5 pl-3 pr-2 text-sm italic text-slate-300">
        {children}
      </blockquote>
    ),
    pre: ({ children }) => (
      <pre className="my-3 overflow-x-auto rounded-lg border border-slate-800 bg-slate-950/90 p-3 font-mono text-xs leading-relaxed text-slate-200 [&_code]:bg-transparent [&_code]:p-0 [&_code]:text-inherit">
        {children}
      </pre>
    ),
    code: ({ children }) => (
      <code className="rounded bg-slate-800/90 px-1.5 py-0.5 font-mono text-xs text-indigo-200">
        {children}
      </code>
    ),
    hr: () => <hr className="my-4 border-slate-800" />,
    strong: ({ children }) => <strong className="font-semibold text-white">{children}</strong>,
    em: ({ children }) => <em className="italic text-slate-200">{children}</em>,
    del: ({ children }) => <del className="text-slate-500 line-through">{children}</del>,
    table: ({ children }) => (
      <div className="my-3 overflow-x-auto rounded-lg border border-slate-800">
        <table className="min-w-full divide-y divide-slate-800 text-left text-xs text-slate-300">
          {children}
        </table>
      </div>
    ),
    thead: ({ children }) => <thead className="bg-slate-900/90 text-slate-200">{children}</thead>,
    tbody: ({ children }) => (
      <tbody className="divide-y divide-slate-800/70 bg-slate-950/40">{children}</tbody>
    ),
    tr: ({ children }) => <tr className="transition-colors hover:bg-slate-900/40">{children}</tr>,
    th: ({ children }) => (
      <th className="px-3 py-2 font-semibold uppercase tracking-wider text-slate-300">
        {children}
      </th>
    ),
    td: ({ children }) => <td className="px-3 py-2 align-top text-slate-300">{children}</td>,
    a: ({ href, title, children }) => {
      const safeHref = sanitizeHttpUrl(href);
      if (!safeHref) {
        return (
          <span
            className="cursor-not-allowed rounded bg-amber-950/60 px-1.5 py-0.5 text-amber-300 underline decoration-dotted"
            title={
              title
                ? `Blocked unsafe link (${title})`
                : 'Blocked unsafe or non-HTTP(S) link'
            }
            data-blocked-link="true"
          >
            {children}
          </span>
        );
      }

      return (
        <a
          href={safeHref}
          target="_blank"
          rel="noopener noreferrer"
          title={title}
          className="text-indigo-400 underline decoration-indigo-400/50 underline-offset-2 transition-colors hover:text-indigo-300 hover:decoration-indigo-300"
          onClick={(event) => {
            if (bridge) {
              event.preventDefault();
              void bridge.openExternalUrl(safeHref);
            }
          }}
        >
          {children}
        </a>
      );
    },
    img: ({ src, alt }) => {
      const safeSrc = sanitizeHttpUrl(src);
      if (!safeSrc) {
        return null;
      }
      return (
        <img
          src={safeSrc}
          alt={alt ?? ''}
          loading="lazy"
          className="my-3 max-w-full rounded-lg border border-slate-800"
        />
      );
    },
  };
}

export function SafeMarkdown({ content, className, bridge }: SafeMarkdownProps) {
  if (!content.trim()) {
    return null;
  }

  return (
    <div className={className ? `safe-markdown ${className}` : 'safe-markdown'}>
      <Markdown
        remarkPlugins={[remarkGfm]}
        skipHtml
        urlTransform={safeUrlTransform}
        components={buildComponents(bridge)}
      >
        {content}
      </Markdown>
    </div>
  );
}

export default SafeMarkdown;
