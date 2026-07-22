// 指定 PID のオンスクリーンウィンドウ矩形を "x y w h"（ポイント座標）で出力する。
// 紹介動画の収録（record-sample.sh）で screencapture -R の切り出し領域を決めるために使う。
import CoreGraphics
import Foundation

guard CommandLine.arguments.count > 1, let pid = Int32(CommandLine.arguments[1]) else {
    FileHandle.standardError.write("usage: winbounds <pid>\n".data(using: .utf8)!)
    exit(2)
}
let opts: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
guard let list = CGWindowListCopyWindowInfo(opts, kCGNullWindowID) as? [[String: Any]] else {
    exit(1)
}
for w in list {
    guard let ownerPid = w[kCGWindowOwnerPID as String] as? Int32, ownerPid == pid,
        let layer = w[kCGWindowLayer as String] as? Int, layer == 0,
        let b = w[kCGWindowBounds as String] as? [String: CGFloat]
    else { continue }
    let x = Int(b["X"] ?? 0), y = Int(b["Y"] ?? 0)
    let wd = Int(b["Width"] ?? 0), ht = Int(b["Height"] ?? 0)
    if wd < 200 || ht < 200 { continue } // ツールチップ等の小窓を除外
    print("\(x) \(y) \(wd) \(ht)")
    exit(0)
}
exit(1)
