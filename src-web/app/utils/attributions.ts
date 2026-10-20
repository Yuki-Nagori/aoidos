export type AttributionInlinePart =
  | { kind: "text" | "strong" | "code"; value: string }
  | { kind: "link"; value: string; href: string };

export type AttributionBlock =
  | { kind: "heading"; parts: AttributionInlinePart[] }
  | { kind: "paragraph"; parts: AttributionInlinePart[] }
  | { kind: "list"; items: AttributionInlinePart[][] };

export interface AttributionDocument {
  id: number;
  heading?: string;
  blocks: AttributionBlock[];
}

function inlineParts(source: string): AttributionInlinePart[] {
  const parts: AttributionInlinePart[] = [];
  const syntax = /\[([^\]]+)\]\(([^)\s]+)\)|\*\*([^*]+)\*\*|`([^`]+)`/g;
  let start = 0;
  const append = (part: AttributionInlinePart) => {
    const previous = parts.at(-1);
    if (part.kind === "text" && previous?.kind === "text") {
      previous.value += part.value;
    } else {
      parts.push(part);
    }
  };

  for (const match of source.matchAll(syntax)) {
    const index = match.index;
    const token = match[0];
    if (index > start) append({ kind: "text", value: source.slice(start, index) });

    const [, label, target, strong, code] = match;
    if (label && target) {
      if (/^https?:\/\//i.test(target)) append({ kind: "link", value: label, href: target });
      else append({ kind: "text", value: `${label}（${target}）` });
    } else if (strong) append({ kind: "strong", value: strong });
    else if (code) append({ kind: "code", value: code });

    start = index + token.length;
  }

  if (start < source.length) append({ kind: "text", value: source.slice(start) });
  return parts;
}

function documentBlocks(source: string): AttributionBlock[] {
  const blocks: AttributionBlock[] = [];
  let paragraph: string[] = [];
  let list: string[] = [];

  const flushParagraph = () => {
    if (paragraph.length)
      blocks.push({ kind: "paragraph", parts: inlineParts(paragraph.join(" ")) });
    paragraph = [];
  };
  const flushList = () => {
    if (list.length) blocks.push({ kind: "list", items: list.map(inlineParts) });
    list = [];
  };

  for (const line of source.split("\n")) {
    const heading = /^#{2,6}\s+(.+)$/.exec(line.trim());
    const item = /^[-*]\s+(.+)$/.exec(line.trim());

    if (heading) {
      flushParagraph();
      flushList();
      blocks.push({ kind: "heading", parts: inlineParts(heading[1]!) });
    } else if (item) {
      flushParagraph();
      list.push(item[1]!);
    } else if (!line.trim() || /^---+$/.test(line.trim())) {
      flushParagraph();
      flushList();
    } else {
      flushList();
      paragraph.push(line.trim());
    }
  }

  flushParagraph();
  flushList();
  return blocks;
}

/** 把随剧本提供的 Markdown 署名拆为安全的文本节点，不把原文当 HTML 执行。 */
export function parseAttributionDocuments(source: string): AttributionDocument[] {
  const normalized = source.replace(/\r\n?/g, "\n").trim();
  return normalized
    .split(/\n{2,}(?=#\s)/)
    .filter(Boolean)
    .map((document, id) => {
      const heading = /^#\s+([^\n]+)(?:\n|$)/.exec(document);
      const body = heading ? document.slice(heading[0].length).trim() : document;
      return {
        id,
        ...(heading ? { heading: heading[1] } : {}),
        blocks: documentBlocks(body),
      };
    });
}
