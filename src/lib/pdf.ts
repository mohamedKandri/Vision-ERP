// Minimal PDF writer: one JPEG image per page, filling the page. The pages are drawn on a
// canvas first, so the browser shapes Arabic text and no font has to be embedded.

export interface PdfPage {
  jpeg: Uint8Array; // JPEG file bytes
  width: number; // pixels
  height: number;
}

/** A4 portrait in points (1/72 inch). */
export const A4 = { w: 595.28, h: 841.89 };

export function buildPdf(pages: PdfPage[], size = A4): Uint8Array {
  const enc = new TextEncoder();
  const chunks: Uint8Array[] = [];
  let length = 0;
  const offsets: number[] = []; // byte offset of object n at [n - 1]
  const push = (b: Uint8Array | string) => {
    const bytes = typeof b === "string" ? enc.encode(b) : b;
    chunks.push(bytes);
    length += bytes.length;
  };
  const obj = (n: number, body: () => void) => {
    offsets[n - 1] = length;
    push(`${n} 0 obj\n`);
    body();
    push("\nendobj\n");
  };

  push("%PDF-1.4\n");
  push(new Uint8Array([0x25, 0xe2, 0xe3, 0xcf, 0xd3, 0x0a])); // "%" + 4 bytes > 127: marks the file as binary
  // 1 catalog, 2 pages, then 3 objects per page: page, content, image.
  const pageIds = pages.map((_, i) => 3 + i * 3);
  obj(1, () => push("<< /Type /Catalog /Pages 2 0 R >>"));
  obj(2, () => push(`<< /Type /Pages /Count ${pages.length} /Kids [${pageIds.map((id) => `${id} 0 R`).join(" ")}] >>`));
  const w = size.w.toFixed(2);
  const h = size.h.toFixed(2);
  pages.forEach((p, i) => {
    const [page, content, image] = [pageIds[i], pageIds[i] + 1, pageIds[i] + 2];
    obj(page, () =>
      push(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${w} ${h}] /Resources << /XObject << /Im${i} ${image} 0 R >> >> /Contents ${content} 0 R >>`),
    );
    const draw = `q ${w} 0 0 ${h} 0 0 cm /Im${i} Do Q`;
    obj(content, () => push(`<< /Length ${draw.length} >>\nstream\n${draw}\nendstream`));
    obj(image, () => {
      push(`<< /Type /XObject /Subtype /Image /Width ${p.width} /Height ${p.height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length ${p.jpeg.length} >>\nstream\n`);
      push(p.jpeg);
      push("\nendstream");
    });
  });
  const xref = length;
  const count = offsets.length + 1;
  push(`xref\n0 ${count}\n0000000000 65535 f \n`);
  for (const o of offsets) push(`${String(o).padStart(10, "0")} 00000 n \n`);
  push(`trailer\n<< /Size ${count} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`);

  const out = new Uint8Array(length);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}
