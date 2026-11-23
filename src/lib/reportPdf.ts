// Draw report tables onto A4 canvas pages, then pack them into a PDF (lib/pdf.ts).

import { buildPdf, type PdfPage } from "./pdf";
import { cellText, type Table } from "./report";
import type { Lang } from "./rpc";

const W = 1240; // A4 at 150 dpi
const H = 1754;
const M = 70; // margin
const ROW = 34;

export interface DocHead {
  shop: string;
  title: string;
  lines: string[]; // range, generated at...
  footer: (page: number, pages: number) => string;
}

/** Wrap the leading number (with sign and %) in a left-to-right isolate (U+2066 ... U+2069). */
export function isolateNumber(s: string): string {
  return s.replace(/^[-+]?[\d\s.,\u00a0\u202f]*\d(\s?%)?/, (m) => `\u2066${m}\u2069`);
}

function dataUrlBytes(url: string): Uint8Array {
  const b = atob(url.slice(url.indexOf(",") + 1));
  const out = new Uint8Array(b.length);
  for (let i = 0; i < b.length; i++) out[i] = b.charCodeAt(i);
  return out;
}

export function renderReportPdf(head: DocHead, tables: Table[], lang: Lang): Uint8Array {
  const rtl = lang === "ar";
  const family = rtl ? '"Noto Sans Arabic", Tahoma, Arial, sans-serif' : '"IBM Plex Sans", Arial, sans-serif';
  const font = (px: number, bold = false) => `${bold ? 700 : 400} ${px}px ${family}`;
  const canvases: HTMLCanvasElement[] = [];
  let ctx!: CanvasRenderingContext2D;
  let y = 0;

  // In RTL the whole layout is mirrored: x measured from the right edge.
  const X = (x: number, w = 0) => (rtl ? W - x - w : x);
  // `num`: in Arabic pages the number is isolated left-to-right so "-12,00" and "79,3 %" keep their
  // order, while the currency stays right-to-left (same as the screen: "د.م. -12,00").
  const text = (s: string, x: number, yy: number, w: number, align: "start" | "end", f: string, color = "#111", num = false) => {
    ctx.font = f;
    ctx.fillStyle = color;
    ctx.direction = rtl ? "rtl" : "ltr";
    let v = rtl && num ? isolateNumber(s) : s;
    if (ctx.measureText(v).width > w) {
      while (v.length > 1 && ctx.measureText(v + "…").width > w) v = v.slice(0, -1);
      v += "…";
    }
    const left = X(x, w);
    const atRight = (align === "end") !== rtl;
    ctx.textAlign = atRight ? "right" : "left";
    ctx.fillText(v, atRight ? left + w : left, yy);
  };
  const newPage = () => {
    const c = document.createElement("canvas");
    c.width = W;
    c.height = H;
    ctx = c.getContext("2d")!;
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, W, H);
    ctx.textBaseline = "middle";
    canvases.push(c);
    y = M;
  };
  const room = (h: number) => {
    if (y + h > H - M - 30) newPage();
  };

  newPage();
  text(head.shop, M, y + 18, W - 2 * M, "start", font(34, true));
  y += 50;
  text(head.title, M, y + 14, W - 2 * M, "start", font(26, true), "#333");
  y += 40;
  for (const l of head.lines) {
    text(l, M, y + 12, W - 2 * M, "start", font(20), "#555");
    y += 28;
  }
  y += 16;

  for (const tb of tables) {
    // Column widths: numbers sized to their widest value, text columns share the rest.
    const inner = W - 2 * M;
    ctx.font = font(19);
    const all = [tb.columns.map((c) => c.label), ...tb.rows.map((r) => r.map((c) => cellText(c, lang))), ...(tb.total ? [tb.total.map((c) => cellText(c, lang))] : [])];
    const widths = tb.columns.map((c, i) =>
      c.num ? Math.min(220, Math.max(...all.map((r) => ctx.measureText(r[i] ?? "").width)) + 24) : 0,
    );
    const textCols = tb.columns.filter((c) => !c.num).length || 1;
    const free = Math.max(120, inner - widths.reduce((s, w) => s + w, 0));
    tb.columns.forEach((c, i) => {
      if (!c.num) widths[i] = free / textCols;
    });
    const scale = inner / widths.reduce((s, w) => s + w, 0);
    const xs: number[] = [];
    widths.forEach((w, i) => {
      widths[i] = w * scale;
      xs[i] = M + widths.slice(0, i).reduce((s, v) => s + v, 0);
    });
    const header = () => {
      ctx.fillStyle = "#f1f1f1";
      ctx.fillRect(M, y, inner, ROW);
      tb.columns.forEach((c, i) => text(c.label, xs[i] + 6, y + ROW / 2, widths[i] - 12, c.num ? "end" : "start", font(17, true), "#333"));
      y += ROW;
    };
    const row = (cells: string[], bold: boolean) => {
      room(ROW);
      if (y === M) header();
      cells.forEach((s, i) => text(s, xs[i] + 6, y + ROW / 2, widths[i] - 12, tb.columns[i]?.num ? "end" : "start", font(18, bold), "#111", !!tb.columns[i]?.num));
      ctx.fillStyle = "#e3e3e3";
      ctx.fillRect(M, y + ROW - 1, inner, 1);
      y += ROW;
    };

    room(ROW * 4);
    text(tb.title, M, y + 18, inner, "start", font(24, true));
    y += 44;
    header();
    if (!tb.rows.length) {
      row(["—", ...tb.columns.slice(1).map(() => "")], false);
    }
    for (const r of tb.rows) row(r.map((c) => cellText(c, lang)), false);
    if (tb.total) row(tb.total.map((c) => cellText(c, lang)), true);
    if (tb.note) {
      room(30);
      text(tb.note, M, y + 16, inner, "start", font(15), "#666");
      y += 30;
    }
    y += 24;
  }

  const pages: PdfPage[] = canvases.map((c, i) => {
    ctx = c.getContext("2d")!;
    text(head.footer(i + 1, canvases.length), M, H - M / 2, W - 2 * M, "end", font(15), "#777");
    return { jpeg: dataUrlBytes(c.toDataURL("image/jpeg", 0.85)), width: W, height: H };
  });
  return buildPdf(pages);
}
