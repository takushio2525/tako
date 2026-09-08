// クリックの注釈素材（ポインタ + クリック波紋）を PNG で吐く（Issue #1081 の GUI モード章）。
//
// なぜ後処理で描くのか: 収録は `screencapture -x -o -l<windowID>`（ウインドウ単体）で、
// **カーソルは 1 ピクセルも写らない**。一方 GUI モード章は「どのボタンを押したか」が
// 本題なので、押した位置が見えないと章として成立しない。実クリックは本物
// （click.swift = 実 OS マウスと同じ CGEvent）で行い、その座標と時刻を
// `<scene>-clicks.tsv` に残して annotate-clicks.sh がここで作った絵を重ねる。
//
// ポインタは macOS の矢印カーソルと同じ形（白抜き + 黒縁）を CoreGraphics で描く。
// `NSCursor.arrow.image` は NSApplication の無いコマンドラインツールでは
// サイズ 0 になり使えない（2026-09-09 実測）ので自前で持つ。
//
// 使い方: pointer <出力ディレクトリ> [--scale 2]
// 出力: pointer.png / ring1.png / ring2.png / ring3.png と、標準出力へ
//       `pointer <w> <h> <hotspot_x> <hotspot_y>` / `ring <w> <h>` の 2 行
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let args = Array(CommandLine.arguments.dropFirst())
guard let outDir = args.first else {
    FileHandle.standardError.write("usage: pointer <out-dir> [--scale N]\n".data(using: .utf8)!)
    exit(2)
}
var scale: CGFloat = 2
if let idx = args.firstIndex(of: "--scale"), idx + 1 < args.count, let v = Double(args[idx + 1]) {
    scale = CGFloat(v)
}
try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)

func die(_ message: String) -> Never {
    FileHandle.standardError.write("ERROR: \(message)\n".data(using: .utf8)!)
    exit(1)
}

func writePNG(_ image: CGImage, to path: String) {
    let url = URL(fileURLWithPath: path) as CFURL
    guard let dest = CGImageDestinationCreateWithURL(url, UTType.png.identifier as CFString, 1, nil)
    else { die("PNG を作れない: \(path)") }
    CGImageDestinationAddImage(dest, image, nil)
    if !CGImageDestinationFinalize(dest) { die("PNG を書けない: \(path)") }
}

func context(_ w: Int, _ h: Int) -> CGContext {
    guard w > 0, h > 0,
        let space = CGColorSpace(name: CGColorSpace.sRGB),
        let ctx = CGContext(
            data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0, space: space,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
    else { die("ビットマップを作れない（\(w)x\(h)）") }
    return ctx
}

// ── ポインタ ─────────────────────────────────────────────────────
// macOS の矢印カーソルと同じ寸法（16x24pt 相当）。原点は左上（= ホットスポット）で、
// CoreGraphics は左下原点なので y を反転して描く
let ptW = Int((17 * scale).rounded())
let ptH = Int((25 * scale).rounded())
let pctx = context(ptW, ptH)
// 左上原点で書けるように座標系を反転する
pctx.translateBy(x: 0, y: CGFloat(ptH))
pctx.scaleBy(x: scale, y: -scale)

let arrow: [(CGFloat, CGFloat)] = [
    (1.0, 1.0), (1.0, 18.6), (5.3, 14.4), (8.2, 21.6), (11.0, 20.4), (8.1, 13.3), (14.2, 13.3),
]
let path = CGMutablePath()
path.move(to: CGPoint(x: arrow[0].0, y: arrow[0].1))
for p in arrow.dropFirst() { path.addLine(to: CGPoint(x: p.0, y: p.1)) }
path.closeSubpath()

pctx.setShadow(offset: CGSize(width: 0.6, height: -0.6), blur: 1.6 * scale)
pctx.addPath(path)
pctx.setFillColor(red: 1, green: 1, blue: 1, alpha: 1)
pctx.fillPath()
pctx.setShadow(offset: .zero, blur: 0)
pctx.addPath(path)
pctx.setStrokeColor(red: 0.05, green: 0.06, blue: 0.09, alpha: 1)
pctx.setLineWidth(1.1)
pctx.strokePath()

guard let pointerImage = pctx.makeImage() else { die("ポインタを描けない") }
writePNG(pointerImage, to: (outDir as NSString).appendingPathComponent("pointer.png"))

// ── クリック波紋（3 枚。押した直後に順に重ねると広がって見える）────────
// tako のアクセント色に近い青。絵文字・文字は使わない
let ringMax: CGFloat = 46 * scale
let ringBox = Int(ringMax * 2)
for (idx, spec) in [(0.34, 0.85), (0.62, 0.55), (0.95, 0.26)].enumerated() {
    let (radiusRatio, alpha) = spec
    let ctx = context(ringBox, ringBox)
    let r = ringMax * CGFloat(radiusRatio)
    let center = CGFloat(ringBox) / 2
    let rect = CGRect(x: center - r, y: center - r, width: r * 2, height: r * 2)
    ctx.setFillColor(red: 0.42, green: 0.62, blue: 1.0, alpha: CGFloat(alpha) * 0.18)
    ctx.fillEllipse(in: rect)
    ctx.setLineWidth(3.0 * scale)
    ctx.setStrokeColor(red: 0.42, green: 0.62, blue: 1.0, alpha: CGFloat(alpha))
    ctx.strokeEllipse(in: rect)
    guard let ringImage = ctx.makeImage() else { die("波紋を描けない") }
    writePNG(ringImage, to: (outDir as NSString).appendingPathComponent("ring\(idx + 1).png"))
}

print("pointer \(ptW) \(ptH) \(Int(scale.rounded())) \(Int(scale.rounded()))")
print("ring \(ringBox) \(ringBox)")
