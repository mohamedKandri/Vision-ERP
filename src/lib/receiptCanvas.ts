// Draw a receipt (ReceiptLine[]) onto a canvas and return a PNG data URL.
// Used for Arabic receipts and for printers set to "graphic mode": the browser shapes
// Arabic text correctly, and the Host prints the image as an ESC/POS raster.

import QRCode from "qrcode";
import type { ReceiptLine } from "./receipt";

export async function renderReceiptPng(lines: ReceiptLine[], dots: 384 | 576, rtl: boolean): Promise<string> {
  const pad = 8;
  const fs = dots === 384 ? 22 : 26; // base font size in px (≈ printer font A)
  const lh = Math.round(fs * 1.3);
  const font = (bold: boolean, big: boolean) =>
    `${bold ? "700" : "400"} ${big ? fs * 1.6 : fs}px ${rtl ? '"Noto Sans Arabic", Tahoma, Arial' : '"Courier New", monospace'}`;

  // Pre-load images so heights are known.
  const images = new Map<number, HTMLImageElement | HTMLCanvasElement>();
  for (const [i, l] of lines.entries()) {
    if (l.kind === "logo") images.set(i, await loadImage(l.src).catch(() => document.createElement("canvas")));
    if (l.kind === "qr") {
      const c = document.createElement("canvas");
      await QRCode.toCanvas(c, l.data, { margin: 1, width: Math.round(dots * 0.4) });
      images.set(i, c);
    }
  }

  const measure = document.createElement("canvas").getContext("2d")!;
  const wrapPx = (text: string, f: string, width: number): string[] => {
    measure.font = f;
    const out: string[] = [];
    // Never split an amount from its currency ("86,36 د.م.").
    const glued = text.replace(/(\d) (د\.م\.|DH)/g, "$1 $2");
    for (const para of glued.split("\n")) {
      let line = "";
      for (const word of para.split(/ +/).filter(Boolean)) {
        const next = line ? `${line} ${word}` : word;
        if (measure.measureText(next).width <= width || !line) line = next;
        else {
          out.push(line);
          line = word;
        }
      }
      out.push(line);
    }
    return out;
  };

  // Layout pass.
  type Op = { y: number; draw: (ctx: CanvasRenderingContext2D, y: number) => void };
  const ops: Op[] = [];
  let y = pad;
  const inner = dots - pad * 2;
  for (const [i, l] of lines.entries()) {
    switch (l.kind) {
      case "rule":
        ops.push({ y, draw: (c, yy) => { c.setLineDash([6, 4]); c.beginPath(); c.moveTo(pad, yy + lh / 2); c.lineTo(dots - pad, yy + lh / 2); c.stroke(); c.setLineDash([]); } });
        y += lh;
        break;
      case "feed":
        y += Math.round(lh / 2);
        break;
      case "logo":
      case "qr": {
        const img = images.get(i)!;
        const w = Math.min(img.width, l.kind === "logo" ? dots * 0.6 : dots * 0.5);
        const h = img.width ? (img.height * w) / img.width : 0;
        ops.push({ y, draw: (c, yy) => c.drawImage(img, (dots - w) / 2, yy, w, h) });
        y += h + 6;
        break;
      }
      case "text": {
        const f = font(!!l.bold, !!l.big);
        const h = l.big ? Math.round(lh * 1.6) : lh;
        for (const part of wrapPx(l.text, f, inner)) {
          ops.push({
            y,
            draw: (c, yy) => {
              c.font = f;
              const align = l.align === "center" ? "center" : l.align === "right" ? "end" : "start";
              c.textAlign = align;
              const x = align === "center" ? dots / 2 : (align === "start") !== rtl ? pad : dots - pad;
              c.fillText(part, x, yy + h * 0.78);
            },
          });
          y += h;
        }
        break;
      }
      case "row": {
        const f = font(!!l.bold, !!l.big);
        const h = l.big ? Math.round(lh * 1.6) : lh;
        measure.font = f;
        const rightW = measure.measureText(l.right).width;
        const parts = wrapPx(l.left, f, inner - rightW - 12);
        parts.forEach((part, j) => {
          const last = j === parts.length - 1;
          ops.push({
            y,
            draw: (c, yy) => {
              c.font = f;
              c.textAlign = "start";
              c.fillText(part, rtl ? dots - pad : pad, yy + h * 0.78);
              if (last) {
                // Amount on the far side. In Arabic it is drawn RTL so "70,00 د.م." reads
                // naturally (number first, currency after, trailing dot in the right place).
                c.textAlign = rtl ? "left" : "right";
                c.fillText(l.right, rtl ? pad : dots - pad, yy + h * 0.78);
              }
            },
          });
          y += h;
        });
        break;
      }
    }
  }
  y += pad;

  const canvas = document.createElement("canvas");
  canvas.width = dots;
  canvas.height = Math.ceil(y);
  const ctx = canvas.getContext("2d")!;
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = "#000";
  ctx.strokeStyle = "#000";
  ctx.lineWidth = 2;
  ctx.direction = rtl ? "rtl" : "ltr";
  for (const op of ops) op.draw(ctx, op.y);
  return canvas.toDataURL("image/png");
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = src;
  });
}
