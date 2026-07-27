<#
.SYNOPSIS
    tako の Windows 用アプリアイコン（マルチサイズ .ico）を生成する。

.DESCRIPTION
    採用済みの A 案（assets/icon/icon-a.svg を書き出した preview/icon-a-1024.png）を
    元に、16〜256 px のマルチサイズ .ico を組み立てる。

    - 16〜128 px は BMP（BITMAPINFOHEADER + 32bpp BGRA + AND マスク）で格納する。
      PNG 圧縮エントリは Windows Vista 以降でしか読めず、読み手によっては 256 px 以外の
      PNG エントリを無視するため、小サイズは互換性の高い BMP に倒す。
    - 256 px だけ PNG で格納する（BMP だと 256 KiB 超になり、こちらは PNG が慣例）。

    依存は .NET の System.Drawing のみで、ImageMagick / Python / librsvg は不要。
    Windows 専用（System.Drawing.Common は .NET 6 以降 Windows でのみ動く）。

.PARAMETER Source
    元画像（PNG）。既定は assets/icon/preview/icon-a-1024.png。
    SVG から作り直す場合は assets/icon/README.md の rsvg-convert 手順で PNG を
    書き出してから、このスクリプトへ渡す。

.PARAMETER Output
    出力先 .ico。既定は assets/icon/tako.ico。

.EXAMPLE
    pwsh -File installer/windows/make-icon.ps1
#>
[CmdletBinding()]
param(
    [string]$Source,
    [string]$Output,
    [int[]]$Sizes = @(16, 24, 32, 48, 64, 128, 256),
    [int[]]$PngSizes = @(256)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Drawing

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $Source) { $Source = Join-Path $repoRoot 'assets/icon/preview/icon-a-1024.png' }
if (-not $Output) { $Output = Join-Path $repoRoot 'assets/icon/tako.ico' }

if (-not (Test-Path -LiteralPath $Source)) {
    throw "元画像が見つからない: $Source"
}

# 縮小。アルファを保ったまま落としたいので CompositingMode は SourceCopy にする
# （既定の SourceOver だと透明な下地へ合成されて縁が濁る）。
function Resize-IconBitmap {
    param([System.Drawing.Bitmap]$Src, [int]$Size)

    $dst = [System.Drawing.Bitmap]::new($Size, $Size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($dst)
    $attr = [System.Drawing.Imaging.ImageAttributes]::new()
    try {
        $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
        $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        # 端のサンプリングが画像外へ回り込んで薄い枠が出るのを防ぐ
        $attr.SetWrapMode([System.Drawing.Drawing2D.WrapMode]::TileFlipXY)

        $rect = [System.Drawing.Rectangle]::new(0, 0, $Size, $Size)
        $g.DrawImage($Src, $rect, 0, 0, $Src.Width, $Src.Height,
            [System.Drawing.GraphicsUnit]::Pixel, $attr)
    } finally {
        $attr.Dispose()
        $g.Dispose()
    }
    return $dst
}

# ICONDIRENTRY が指す実体を BMP 形式（ファイルヘッダ無し・高さは 2 倍・行は下から上）で作る
function ConvertTo-IconDib {
    param([System.Drawing.Bitmap]$Bmp)

    $width = $Bmp.Width
    $height = $Bmp.Height
    $rect = [System.Drawing.Rectangle]::new(0, 0, $width, $height)
    $locked = $Bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        $stride = $locked.Stride
        $raw = [byte[]]::new($stride * $height)
        [System.Runtime.InteropServices.Marshal]::Copy($locked.Scan0, $raw, 0, $raw.Length)
    } finally {
        $Bmp.UnlockBits($locked)
    }

    # AND マスクは 1bpp・行 4 バイト境界。32bpp のアルファがあるので中身は全 0（= 不透明扱い）で良い
    $maskStride = [int]([math]::Floor(($width + 31) / 32) * 4)
    $xorSize = $width * 4 * $height

    $ms = [System.IO.MemoryStream]::new()
    $writer = [System.IO.BinaryWriter]::new($ms)
    try {
        # BITMAPINFOHEADER
        $writer.Write([uint32]40)
        $writer.Write([int32]$width)
        $writer.Write([int32]($height * 2))   # XOR + AND の合計高さ
        $writer.Write([uint16]1)              # planes
        $writer.Write([uint16]32)             # bit count
        $writer.Write([uint32]0)              # BI_RGB
        $writer.Write([uint32]($xorSize + $maskStride * $height))
        $writer.Write([int32]0)               # X pels per meter
        $writer.Write([int32]0)               # Y pels per meter
        $writer.Write([uint32]0)              # clr used
        $writer.Write([uint32]0)              # clr important

        for ($y = $height - 1; $y -ge 0; $y--) {
            $writer.Write($raw, $y * $stride, $width * 4)
        }
        $writer.Write([byte[]]::new($maskStride * $height), 0, $maskStride * $height)
        $writer.Flush()
        return $ms.ToArray()
    } finally {
        $writer.Dispose()
        $ms.Dispose()
    }
}

function ConvertTo-PngBytes {
    param([System.Drawing.Bitmap]$Bmp)

    $ms = [System.IO.MemoryStream]::new()
    try {
        $Bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        return $ms.ToArray()
    } finally {
        $ms.Dispose()
    }
}

$src = [System.Drawing.Bitmap]::new($Source)
$entries = [System.Collections.Generic.List[object]]::new()
try {
    Write-Host "元画像: $Source ($($src.Width)x$($src.Height))"
    foreach ($size in ($Sizes | Sort-Object -Unique)) {
        if ($size -lt 1 -or $size -gt 256) { throw "サイズは 1〜256 の範囲: $size" }
        $bmp = Resize-IconBitmap -Src $src -Size $size
        try {
            $usePng = $PngSizes -contains $size
            $data = if ($usePng) { ConvertTo-PngBytes -Bmp $bmp } else { ConvertTo-IconDib -Bmp $bmp }
        } finally {
            $bmp.Dispose()
        }
        $entries.Add([pscustomobject]@{ Size = $size; Png = $usePng; Data = $data })
        Write-Host ("  {0,3}px {1,-4} {2,7} bytes" -f $size, $(if ($usePng) { 'png' } else { 'bmp' }), $data.Length)
    }
} finally {
    $src.Dispose()
}

$out = [System.IO.MemoryStream]::new()
$writer = [System.IO.BinaryWriter]::new($out)
try {
    # ICONDIR
    $writer.Write([uint16]0)                 # reserved
    $writer.Write([uint16]1)                 # type = icon
    $writer.Write([uint16]$entries.Count)

    $offset = 6 + 16 * $entries.Count
    foreach ($e in $entries) {
        # 256px は 0 で表す（1 バイトに収まらないため）
        $dim = [byte]$(if ($e.Size -ge 256) { 0 } else { $e.Size })
        $writer.Write($dim)                  # width
        $writer.Write($dim)                  # height
        $writer.Write([byte]0)               # color count（32bpp なので 0）
        $writer.Write([byte]0)               # reserved
        $writer.Write([uint16]1)             # planes
        $writer.Write([uint16]32)            # bit count
        $writer.Write([uint32]$e.Data.Length)
        $writer.Write([uint32]$offset)
        $offset += $e.Data.Length
    }
    foreach ($e in $entries) {
        $writer.Write($e.Data, 0, $e.Data.Length)
    }
    $writer.Flush()

    $outDir = Split-Path -Parent $Output
    if ($outDir -and -not (Test-Path -LiteralPath $outDir)) {
        New-Item -ItemType Directory -Path $outDir | Out-Null
    }
    [System.IO.File]::WriteAllBytes($Output, $out.ToArray())
} finally {
    $writer.Dispose()
    $out.Dispose()
}

# 書いたものを読み直して自己検証する（壊れた .ico は ISCC が黙って通すことがある）。
# ICONDIR を自前で読み直し、各エントリの実体が宣言どおりの寸法かまで突き合わせる。
$bytes = [System.IO.File]::ReadAllBytes($Output)
if ($bytes.Length -lt 6) { throw "検証失敗: ファイルが小さすぎる" }
if ([BitConverter]::ToUInt16($bytes, 0) -ne 0 -or [BitConverter]::ToUInt16($bytes, 2) -ne 1) {
    throw "検証失敗: ICONDIR のヘッダが不正"
}
$count = [BitConverter]::ToUInt16($bytes, 4)
if ($count -ne $entries.Count) { throw "検証失敗: エントリ数 $count（期待 $($entries.Count)）" }

$expected = @($Sizes | Sort-Object -Unique)
for ($i = 0; $i -lt $count; $i++) {
    $base = 6 + 16 * $i
    $declared = $bytes[$base]
    $size = if ($declared -eq 0) { 256 } else { [int]$declared }
    if ($size -ne $expected[$i]) { throw "検証失敗: エントリ $i の宣言サイズ $size（期待 $($expected[$i])）" }

    $len = [int][BitConverter]::ToUInt32($bytes, $base + 8)
    $off = [int][BitConverter]::ToUInt32($bytes, $base + 12)
    if ($off -lt (6 + 16 * $count) -or ($off + $len) -gt $bytes.Length) {
        throw "検証失敗: エントリ $i のオフセット/長さがファイル外を指している"
    }

    # PNG（\x89PNG）なら IHDR、そうでなければ BITMAPINFOHEADER から実寸を読む
    $p = [int]$off
    if ($bytes[$p] -eq 0x89 -and $bytes[$p + 1] -eq 0x50) {
        # IHDR の幅・高さはビッグエンディアン。
        # PowerShell の -shl は Byte のまま計算して 8 以上のシフトが 0 に潰れるので [int] へ上げる
        $w = ([int]$bytes[$p + 16] -shl 24) -bor ([int]$bytes[$p + 17] -shl 16) -bor ([int]$bytes[$p + 18] -shl 8) -bor [int]$bytes[$p + 19]
        $h = ([int]$bytes[$p + 20] -shl 24) -bor ([int]$bytes[$p + 21] -shl 16) -bor ([int]$bytes[$p + 22] -shl 8) -bor [int]$bytes[$p + 23]
    }
    else {
        $w = [BitConverter]::ToInt32($bytes, $p + 4)
        $h = [int]([BitConverter]::ToInt32($bytes, $p + 8) / 2)
    }
    if ($w -ne $size -or $h -ne $size) {
        throw "検証失敗: エントリ $i の実体が ${w}x${h}（宣言 ${size}x${size}）"
    }
}

# GDI+ でも読めることを確認する。System.Drawing は PNG 圧縮エントリを読めないため
# （.NET の既知の制限。Windows シェル / Inno Setup 側は 256px PNG を扱える）
# BMP で格納したサイズだけを対象にする。
$verify = [System.Drawing.Icon]::new($Output)
try {
    foreach ($size in ($expected | Where-Object { $PngSizes -notcontains $_ })) {
        $picked = [System.Drawing.Icon]::new($verify, $size, $size)
        try {
            if ($picked.Width -ne $size -or $picked.Height -ne $size) {
                throw "検証失敗: GDI+ が ${size}px を要求されて $($picked.Width)x$($picked.Height) を返した"
            }
        } finally {
            $picked.Dispose()
        }
    }
} finally {
    $verify.Dispose()
}

$len = (Get-Item -LiteralPath $Output).Length
Write-Host "生成: $Output ($len bytes, $($entries.Count) エントリ) — 全サイズの読み出しを検証済み"
