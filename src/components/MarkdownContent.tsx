import { isValidElement, memo, useDeferredValue, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

type CopyTextButtonProps = {
  text: string;
  label: string;
  className?: string;
};

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    const textarea = document.createElement("textarea");
    textarea.value = text;
    textarea.setAttribute("readonly", "");
    textarea.style.position = "fixed";
    textarea.style.opacity = "0";
    document.body.append(textarea);
    textarea.select();
    const copied = document.execCommand("copy");
    textarea.remove();
    if (!copied) throw new Error("Clipboard copy was rejected");
  }
}

export function CopyTextButton({ text, label, className }: CopyTextButtonProps) {
  const [status, setStatus] = useState<"idle" | "copied" | "failed">("idle");
  const resetTimer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(resetTimer.current), []);

  async function handleCopy() {
    window.clearTimeout(resetTimer.current);
    try {
      await copyText(text);
      setStatus("copied");
    } catch {
      setStatus("failed");
    }
    resetTimer.current = window.setTimeout(() => setStatus("idle"), 2_000);
  }

  const visibleLabel = status === "copied" ? "Copied" : status === "failed" ? "Copy failed" : label;
  return (
    <button
      className={className}
      type="button"
      onClick={handleCopy}
      data-copy-status={status}
      aria-live="polite"
    >
      {visibleLabel}
    </button>
  );
}

function nodeText(node: ReactNode): string {
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join("");
  if (isValidElement<{ children?: ReactNode }>(node)) return nodeText(node.props.children);
  return "";
}

function codeLanguage(node: ReactNode): string {
  if (!isValidElement<{ className?: string }>(node)) return "Code";
  const language = node.props.className?.match(/language-([\w-]+)/)?.[1];
  return language ?? "Code";
}

function CodeBlock({ children }: { children?: ReactNode }) {
  const code = nodeText(children).replace(/\n$/, "");
  return (
    <div className="code-block">
      <div className="code-block-header">
        <span>{codeLanguage(children)}</span>
        <CopyTextButton text={code} label="Copy" className="code-copy-action" />
      </div>
      <pre>{children}</pre>
    </div>
  );
}

export function safeMarkdownUrl(url: string): string {
  if (url.startsWith("#") || url.startsWith("/") || url.startsWith("./") || url.startsWith("../")) {
    return url;
  }
  try {
    const parsed = new URL(url);
    return ["http:", "https:", "mailto:"].includes(parsed.protocol) ? url : "";
  } catch {
    return "";
  }
}

type MarkdownContentProps = {
  children: string;
};

export const MarkdownContent = memo(function MarkdownContent({ children }: MarkdownContentProps) {
  const markdown = useDeferredValue(children);
  return (
    <div className="markdown-content">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        urlTransform={safeMarkdownUrl}
        components={{
          a: ({ node: _node, href, children: linkChildren, ...props }) => href ? (
            <a
              {...props}
              href={href}
              target={href.startsWith("#") ? undefined : "_blank"}
              rel={href.startsWith("#") ? undefined : "noopener noreferrer"}
            >
              {linkChildren}
              {!href.startsWith("#") ? <span className="external-link-mark" aria-hidden="true">↗</span> : null}
            </a>
          ) : <span>{linkChildren}</span>,
          img: ({ node: _node, alt }) => (
            <span className="blocked-image" role="note">
              Remote image blocked{alt ? `: ${alt}` : ""}
            </span>
          ),
          pre: ({ node: _node, children: preChildren }) => <CodeBlock>{preChildren}</CodeBlock>,
          table: ({ node: _node, children: tableChildren, ...props }) => (
            <div className="markdown-table-wrap" role="region" aria-label="Scrollable table" tabIndex={0}>
              <table {...props}>{tableChildren}</table>
            </div>
          ),
        }}
      >
        {markdown}
      </ReactMarkdown>
    </div>
  );
});
