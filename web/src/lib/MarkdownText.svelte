<script lang="ts">
  let {
    text = '',
    rawMode = false,
    maxHeight = 'none',
    compact = false
  }: {
    text: string;
    rawMode?: boolean;
    maxHeight?: string;
    compact?: boolean;
  } = $props();

  function escapeHtml(raw: string): string {
    return raw
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function cleanHarnessWrappers(input: string): string {
    if (!input) return '';
    let out = input;
    // Strip outer <USER_REQUEST> tags while keeping the inner prompt
    out = out.replace(/<\/?USER_REQUEST>/gi, '');
    // Collapse <ADDITIONAL_METADATA>...</ADDITIONAL_METADATA> into a single subtle note or remove timestamp noise
    out = out.replace(/<ADDITIONAL_METADATA>[\s\S]*?<\/ADDITIONAL_METADATA>/gi, '');
    return out.trim();
  }

  function formatInlineMarkdown(escapedLine: string): string {
    let s = escapedLine;

    // 1. Extract inline code `...` into placeholders so inner symbols aren't mutated
    const codeSnippets: string[] = [];
    s = s.replace(/`([^`]+)`/g, (_match, code) => {
      const idx = codeSnippets.length;
      codeSnippets.push(`<code class="cw-md-inline-code">${code}</code>`);
      return `\u0000CODE${idx}\u0000`;
    });

    // 2. Markdown Links: [label](url)
    s = s.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_m, label, url) => {
      const cleanUrl = url.replace(/^file:\/\/\/?/, '/');
      const isExternal = /^https?:\/\//i.test(url);
      if (isExternal) {
        return `<a href="${url}" target="_blank" rel="noopener noreferrer" class="cw-md-link">${label}</a>`;
      }
      return `<span class="cw-md-file-link" title="${cleanUrl}">${label}</span>`;
    });

    // 3. Bold + Italic: ***text***
    s = s.replace(/\*\*\*([^*]+)\*\*\*/g, '<strong><em>$1</em></strong>');
    // 4. Bold: **text**
    s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    // 5. Italic: *text* (avoid matching mid-word underscores)
    s = s.replace(/(^|\s)\*([^*\n]+)\*(?=\s|[.,!?:;)]|$)/g, '$1<em>$2</em>');
    // 6. Strikethrough: ~~text~~
    s = s.replace(/~~([^~]+)~~/g, '<del>$1</del>');

    // Restore inline code placeholders
    s = s.replace(/\u0000CODE(\d+)\u0000/g, (_m, idx) => codeSnippets[Number(idx)] || '');
    return s;
  }

  function renderMarkdown(rawInput: string): string {
    const cleaned = cleanHarnessWrappers(rawInput || '');
    if (!cleaned) return '';

    const lines = cleaned.split(/\r?\n/);
    const html: string[] = [];

    let inCodeBlock = false;
    let codeLang = '';
    let codeBuffer: string[] = [];

    let inUl = false;
    let inOl = false;
    let inBlockquote = false;
    let inTable = false;
    let tableHeaderDone = false;

    function closeListsAndBlocks() {
      if (inUl) {
        html.push('</ul>');
        inUl = false;
      }
      if (inOl) {
        html.push('</ol>');
        inOl = false;
      }
      if (inBlockquote) {
        html.push('</blockquote>');
        inBlockquote = false;
      }
      if (inTable) {
        html.push('</tbody></table></div>');
        inTable = false;
        tableHeaderDone = false;
      }
    }

    for (let i = 0; i < lines.length; i++) {
      const rawLine = lines[i];

      // Handle fenced code blocks ```lang
      const fenceMatch = rawLine.match(/^\s*```([\w+-]*)\s*$/);
      if (fenceMatch) {
        closeListsAndBlocks();
        if (!inCodeBlock) {
          inCodeBlock = true;
          codeLang = fenceMatch[1] || 'text';
          codeBuffer = [];
        } else {
          const escapedCode = escapeHtml(codeBuffer.join('\n'));
          const langBadge = codeLang && codeLang !== 'text' ? `<div class="cw-md-code-header">${escapeHtml(codeLang)}</div>` : '';
          html.push(`<div class="cw-md-code-block">${langBadge}<pre><code>${escapedCode}</code></pre></div>`);
          inCodeBlock = false;
          codeLang = '';
          codeBuffer = [];
        }
        continue;
      }

      if (inCodeBlock) {
        codeBuffer.push(rawLine);
        continue;
      }

      const trimmed = rawLine.trim();

      // Empty line
      if (!trimmed) {
        closeListsAndBlocks();
        continue;
      }

      // Horizontal rule
      if (/^(-{3,}|\*{3,}|_{3,}|─{3,})$/.test(trimmed)) {
        closeListsAndBlocks();
        html.push('<hr class="cw-md-hr" />');
        continue;
      }

      // Markdown table row: | col1 | col2 |
      if (trimmed.startsWith('|') && trimmed.endsWith('|') && trimmed.includes('|', 1)) {
        const cells = trimmed
          .slice(1, -1)
          .split('|')
          .map((c) => c.trim());

        // Check if separator row like |---|---|
        if (cells.every((c) => /^:?-{2,}:?$/.test(c))) {
          continue;
        }

        if (!inTable) {
          closeListsAndBlocks();
          inTable = true;
          tableHeaderDone = false;
          html.push('<div class="cw-md-table-wrap"><table class="cw-md-table">');
        }

        if (!tableHeaderDone) {
          html.push(
            '<thead><tr>' +
              cells.map((c) => `<th>${formatInlineMarkdown(escapeHtml(c))}</th>`).join('') +
              '</tr></thead><tbody>'
          );
          tableHeaderDone = true;
        } else {
          html.push(
            '<tr>' +
              cells.map((c) => `<td>${formatInlineMarkdown(escapeHtml(c))}</td>`).join('') +
              '</tr>'
          );
        }
        continue;
      } else if (inTable) {
        closeListsAndBlocks();
      }

      // Headings: # H1 .. #### H4
      const headingMatch = rawLine.match(/^\s*(#{1,4})\s+(.+)$/);
      if (headingMatch) {
        closeListsAndBlocks();
        const level = headingMatch[1].length;
        const content = formatInlineMarkdown(escapeHtml(headingMatch[2]));
        html.push(`<h${level} class="cw-md-h${level}">${content}</h${level}>`);
        continue;
      }

      // Blockquote: > text
      const bqMatch = rawLine.match(/^\s*>\s?(.*)$/);
      if (bqMatch) {
        if (inUl || inOl) closeListsAndBlocks();
        if (!inBlockquote) {
          inBlockquote = true;
          html.push('<blockquote class="cw-md-blockquote">');
        }
        html.push(`<div>${formatInlineMarkdown(escapeHtml(bqMatch[1]))}</div>`);
        continue;
      } else if (inBlockquote) {
        closeListsAndBlocks();
      }

      // Unordered list: - item or * item or • item
      const ulMatch = rawLine.match(/^\s*[-*•]\s+(.+)$/);
      if (ulMatch) {
        if (inOl || inBlockquote) closeListsAndBlocks();
        if (!inUl) {
          inUl = true;
          html.push('<ul class="cw-md-ul">');
        }
        html.push(`<li>${formatInlineMarkdown(escapeHtml(ulMatch[1]))}</li>`);
        continue;
      }

      // Ordered list: 1. item
      const olMatch = rawLine.match(/^\s*\d+[.)]\s+(.+)$/);
      if (olMatch) {
        if (inUl || inBlockquote) closeListsAndBlocks();
        if (!inOl) {
          inOl = true;
          html.push('<ol class="cw-md-ol">');
        }
        html.push(`<li>${formatInlineMarkdown(escapeHtml(olMatch[1]))}</li>`);
        continue;
      }

      // Regular paragraph line
      closeListsAndBlocks();
      html.push(`<p class="cw-md-p">${formatInlineMarkdown(escapeHtml(rawLine))}</p>`);
    }

    if (inCodeBlock) {
      const escapedCode = escapeHtml(codeBuffer.join('\n'));
      html.push(`<div class="cw-md-code-block"><pre><code>${escapedCode}</code></pre></div>`);
    }
    closeListsAndBlocks();

    return html.join('');
  }

  let renderedHtml = $derived(rawMode ? '' : renderMarkdown(text));
</script>

{#if rawMode}
  <pre
    style="margin: 0; white-space: pre-wrap; word-break: break-word; font-family: 'JetBrains Mono', 'Fira Code', monospace; font-size: 12px; color: #d4d4d8; line-height: 1.5; max-height: {maxHeight}; overflow-y: {maxHeight !== 'none' ? 'auto' : 'visible'};"
  >{text}</pre>
{:else}
  <div
    class="cw-md-body {compact ? 'cw-md-compact' : ''}"
    style="max-height: {maxHeight}; overflow-y: {maxHeight !== 'none' ? 'auto' : 'visible'};"
  >
    {@html renderedHtml}
  </div>
{/if}

<style>
  .cw-md-body {
    font-size: 13.5px;
    line-height: 1.6;
    color: #e4e4e7;
    word-break: break-word;
  }

  .cw-md-body.cw-md-compact {
    font-size: 12.5px;
    line-height: 1.5;
  }

  .cw-md-body :global(.cw-md-p) {
    margin: 0 0 8px 0;
  }
  .cw-md-body :global(.cw-md-p:last-child) {
    margin-bottom: 0;
  }

  .cw-md-body :global(strong) {
    color: #ffffff;
    font-weight: 600;
  }

  .cw-md-body :global(em) {
    color: #d4d4d8;
    font-style: italic;
  }

  .cw-md-body :global(.cw-md-h1),
  .cw-md-body :global(.cw-md-h2),
  .cw-md-body :global(.cw-md-h3),
  .cw-md-body :global(.cw-md-h4) {
    color: #fafafa;
    font-weight: 600;
    letter-spacing: -0.01em;
    margin: 12px 0 6px 0;
    line-height: 1.35;
  }
  .cw-md-body :global(.cw-md-h1:first-child),
  .cw-md-body :global(.cw-md-h2:first-child),
  .cw-md-body :global(.cw-md-h3:first-child),
  .cw-md-body :global(.cw-md-h4:first-child) {
    margin-top: 0;
  }
  .cw-md-body :global(.cw-md-h1) {
    font-size: 16px;
    border-bottom: 1px solid #27272a;
    padding-bottom: 4px;
  }
  .cw-md-body :global(.cw-md-h2) {
    font-size: 15px;
  }
  .cw-md-body :global(.cw-md-h3) {
    font-size: 14px;
    color: #93c5fd;
  }
  .cw-md-body :global(.cw-md-h4) {
    font-size: 13px;
    color: #a1a1aa;
  }

  .cw-md-body :global(.cw-md-inline-code) {
    background: #1e202a;
    color: #93c5fd;
    border: 1px solid #2e3244;
    padding: 1px 5px;
    border-radius: 4px;
    font-family: 'JetBrains Mono', 'Fira Code', ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    font-size: 0.88em;
  }

  .cw-md-body :global(.cw-md-code-block) {
    margin: 8px 0;
    background: #0b0c10;
    border: 1px solid #232530;
    border-radius: 7px;
    overflow: hidden;
  }

  .cw-md-body :global(.cw-md-code-header) {
    background: #14161f;
    color: #94a3b8;
    font-size: 10.5px;
    font-family: ui-monospace, monospace;
    padding: 4px 10px;
    border-bottom: 1px solid #232530;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .cw-md-body :global(.cw-md-code-block pre) {
    margin: 0;
    padding: 10px 12px;
    overflow-x: auto;
    font-family: 'JetBrains Mono', 'Fira Code', ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    font-size: 12px;
    line-height: 1.5;
    color: #e2e8f0;
  }

  .cw-md-body :global(.cw-md-ul),
  .cw-md-body :global(.cw-md-ol) {
    margin: 6px 0 8px 0;
    padding-left: 20px;
  }

  .cw-md-body :global(.cw-md-ul li),
  .cw-md-body :global(.cw-md-ol li) {
    margin-bottom: 4px;
    color: #e4e4e7;
  }

  .cw-md-body :global(.cw-md-ul li::marker) {
    color: #60a5fa;
  }

  .cw-md-body :global(.cw-md-ol li::marker) {
    color: #94a3b8;
    font-weight: 600;
  }

  .cw-md-body :global(.cw-md-blockquote) {
    margin: 8px 0;
    padding: 6px 12px;
    border-left: 3px solid #3b82f6;
    background: rgba(59, 130, 246, 0.07);
    border-radius: 0 6px 6px 0;
    color: #cbd5e1;
  }

  .cw-md-body :global(.cw-md-link) {
    color: #60a5fa;
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .cw-md-body :global(.cw-md-file-link) {
    background: rgba(59, 130, 246, 0.12);
    color: #93c5fd;
    border: 1px solid rgba(59, 130, 246, 0.28);
    padding: 0 5px;
    border-radius: 4px;
    font-family: ui-monospace, monospace;
    font-size: 0.9em;
  }

  .cw-md-body :global(.cw-md-hr) {
    border: none;
    border-top: 1px solid #27272a;
    margin: 10px 0;
  }

  .cw-md-body :global(.cw-md-table-wrap) {
    overflow-x: auto;
    margin: 8px 0;
  }

  .cw-md-body :global(.cw-md-table) {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }

  .cw-md-body :global(.cw-md-table th),
  .cw-md-body :global(.cw-md-table td) {
    border: 1px solid #27272a;
    padding: 6px 10px;
    text-align: left;
  }

  .cw-md-body :global(.cw-md-table th) {
    background: #161822;
    color: #f4f4f5;
    font-weight: 600;
  }
</style>
