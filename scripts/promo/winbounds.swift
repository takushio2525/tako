// 指定 PID のオンスクリーンウィンドウの「ウィンドウ ID と矩形」を
// "windowID winX winY winW winH"（座標はポイント）で出力する。
//
// 紹介動画の収録（lib.sh）はこのウィンドウ ID を screencapture -l<id> に渡し、
// **ウィンドウ単体**をキャプチャする。画面全体を撮って切り出す方式（avfoundation +
// crop / screencapture -R）は、収録中に別アプリのウィンドウが対象領域へ重なると
// その内容ごと写り込む事故を起こすため採用しない（2026-07-23 に実際に発生）。
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
        let wid = w[kCGWindowNumber as String] as? Int,
        let b = w[kCGWindowBounds as String] as? [String: CGFloat]
    else { continue }
    let x = Int(b["X"] ?? 0), y = Int(b["Y"] ?? 0)
    let wd = Int(b["Width"] ?? 0), ht = Int(b["Height"] ?? 0)
    if wd < 200 || ht < 200 { continue }  // ツールチップ等の小窓を除外
    print("\(wid) \(x) \(y) \(wd) \(ht)")
    exit(0)
}
exit(1)
