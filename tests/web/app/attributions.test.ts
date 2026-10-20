import { describe, expect, it } from "vitest";
import { parseAttributionDocuments } from "../../../src-web/app/utils/attributions";

describe("剧本署名文档", () => {
  it("将标题、段落、列表和安全外链拆成文本节点", () => {
    const documents = parseAttributionDocuments(
      "# Script License\n\nUse `Bell` under **CC BY 4.0**. See [license](https://example.com/license).\n\n## Scope\n\n- Keep attribution.\n- Do not imply endorsement.",
    );

    expect(documents).toHaveLength(1);
    expect(documents[0]?.heading).toBe("Script License");
    expect(documents[0]?.blocks).toEqual([
      {
        kind: "paragraph",
        parts: [
          { kind: "text", value: "Use " },
          { kind: "code", value: "Bell" },
          { kind: "text", value: " under " },
          { kind: "strong", value: "CC BY 4.0" },
          { kind: "text", value: ". See " },
          { kind: "link", value: "license", href: "https://example.com/license" },
          { kind: "text", value: "." },
        ],
      },
      { kind: "heading", parts: [{ kind: "text", value: "Scope" }] },
      {
        kind: "list",
        items: [
          [{ kind: "text", value: "Keep attribution." }],
          [{ kind: "text", value: "Do not imply endorsement." }],
        ],
      },
    ]);
  });

  it("保留相对文件引用为普通文本，不把它作为外链", () => {
    const [document] = parseAttributionDocuments(
      "# SRD Attribution\n\nSee [the source](SRD-5.2.1.md).",
    );

    expect(document?.blocks[0]).toEqual({
      kind: "paragraph",
      parts: [{ kind: "text", value: "See the source（SRD-5.2.1.md）." }],
    });
  });

  it("按 CRLF 换行拆分多份署名文档", () => {
    const documents = parseAttributionDocuments(
      "# First\r\n\r\nFirst notice.\r\n\r\n# Second\r\n\r\nSecond notice.",
    );

    expect(documents.map(({ heading }) => heading)).toEqual(["First", "Second"]);
    expect(documents.map(({ blocks }) => blocks[0])).toEqual([
      { kind: "paragraph", parts: [{ kind: "text", value: "First notice." }] },
      { kind: "paragraph", parts: [{ kind: "text", value: "Second notice." }] },
    ]);
  });

  it("接受单独的 CR 换行", () => {
    const documents = parseAttributionDocuments("# First\r\n\rNotice.");
    expect(documents.map(({ heading }) => heading)).toEqual(["First"]);
    expect(documents[0]?.blocks).toEqual([
      { kind: "paragraph", parts: [{ kind: "text", value: "Notice." }] },
    ]);
  });
});
