// A small RTF reader: turns an RTF document into plain text, one paragraph per line.
// It understands what question-set files use (groups, paragraphs, tabs, \uN and \'hh escapes,
// the common punctuation words) and skips everything that is not body text (font and colour
// tables, stylesheets, headers and footers, pictures, field instructions, \* destinations).

const CP1252 = {
  0x80: "€", 0x82: "‚", 0x83: "ƒ", 0x84: "„", 0x85: "…", 0x86: "†",
  0x87: "‡", 0x88: "ˆ", 0x89: "‰", 0x8a: "Š", 0x8b: "‹", 0x8c: "Œ",
  0x8e: "Ž", 0x91: "‘", 0x92: "’", 0x93: "“", 0x94: "”", 0x95: "•",
  0x96: "–", 0x97: "—", 0x98: "˜", 0x99: "™", 0x9a: "š", 0x9b: "›",
  0x9c: "œ", 0x9e: "ž", 0x9f: "Ÿ",
};

const SKIPPED_DESTINATIONS = new Set([
  "fonttbl", "colortbl", "stylesheet", "info", "pict", "header", "headerl", "headerr", "headerf",
  "footer", "footerl", "footerr", "footerf", "listtable", "listoverridetable", "rsidtbl",
  "generator", "themedata", "colorschememapping", "latentstyles", "datastore", "xmlnstbl",
  "fldinst", "object", "footnote", "private", "mmathPr", "pgdsctbl", "revtbl", "falt",
  "panose", "fontemb", "filetbl", "wgrffmtfilter", "userprops", "xmlopen", "nonshppict",
]);

const WORD_TEXT = {
  par: "\n", sect: "\n", page: "\n", line: "\n", row: "\n", tab: "\t", cell: "\t",
  emdash: "—", endash: "–", emspace: " ", enspace: " ", qmspace: " ",
  bullet: "•", lquote: "‘", rquote: "’", ldblquote: "“", rdblquote: "”",
};

const hex = value => Number.parseInt(value, 16);

export function rtfToText(rtf) {
  const text = String(rtf);
  let out = "";
  // Each open group remembers whether its text is skipped and how many characters follow a \uN.
  let stack = [];
  let skip = false;
  let unicodeSkip = 1;
  let pendingSkip = 0; // fallback characters still to drop after a \uN
  let i = 0;

  const emit = chunk => { if (!skip && pendingSkip === 0) out += chunk; };

  while (i < text.length) {
    const ch = text[i];
    if (ch === "{") {
      stack.push({ skip, unicodeSkip });
      i++;
      // A group opening with \* or a skipped destination word is dropped whole.
      if (text[i] === "\\" && text[i + 1] === "*") skip = true;
      else if (text[i] === "\\") {
        const word = /^\\([a-zA-Z]+)/.exec(text.slice(i, i + 40));
        if (word && SKIPPED_DESTINATIONS.has(word[1])) skip = true;
      }
    } else if (ch === "}") {
      ({ skip, unicodeSkip } = stack.pop() ?? { skip: false, unicodeSkip: 1 });
      pendingSkip = 0;
      i++;
    } else if (ch === "\\") {
      const next = text[i + 1];
      if (next === undefined) break;
      if (next === "'") {
        const code = hex(text.slice(i + 2, i + 4));
        i += 4;
        if (pendingSkip > 0) { pendingSkip--; continue; }
        emit(CP1252[code] ?? String.fromCharCode(code));
      } else if (/[a-zA-Z]/.test(next)) {
        const match = /^\\([a-zA-Z]+)(-?\d+)? ?/.exec(text.slice(i, i + 40));
        i += match[0].length;
        const word = match[1];
        const value = match[2] === undefined ? null : Number(match[2]);
        if (word === "uc") unicodeSkip = value ?? 1;
        else if (word === "u") {
          if (pendingSkip === 0) emit(String.fromCharCode(value < 0 ? value + 65536 : value));
          pendingSkip = unicodeSkip;
        } else if (word in WORD_TEXT) {
          if (pendingSkip > 0) pendingSkip--;
          else emit(WORD_TEXT[word]);
        }
      } else {
        i += 2;
        if (pendingSkip > 0 && next !== "\\" && next !== "{" && next !== "}") { pendingSkip--; continue; }
        if (next === "~") emit(" ");
        else if (next === "_") emit("-");
        else if (next === "\\" || next === "{" || next === "}") {
          if (pendingSkip > 0) pendingSkip--; else emit(next);
        } else if (next === "\n" || next === "\r") emit("\n");
      }
    } else if (ch === "\r" || ch === "\n") {
      i++;
    } else {
      i++;
      if (pendingSkip > 0) { pendingSkip--; continue; }
      emit(ch);
    }
  }
  return out.replace(/ /g, " ");
}
