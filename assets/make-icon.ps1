# Erzeugt das Programmsymbol (Monitor mit Terminal-Prompt) in allen Größen:
#   icon.ico        – für exe-Ressource und MSI (16…256 px, PNG-komprimiert)
#   icon_256.png    – Vorschau
#   icon_64.rgba    – Rohdaten (RGBA, 64×64) für das Fenstersymbol
Add-Type -AssemblyName System.Drawing

function New-RoundRect([single]$x, [single]$y, [single]$w, [single]$h, [single]$r) {
    $p = New-Object Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc($x, $y, $d, $d, 180, 90); $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90); $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure(); $p
}

function Draw-Icon([int]$size) {
    $bmp = New-Object Drawing.Bitmap $size, $size, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'; $g.PixelOffsetMode = 'HighQuality'
    $g.ScaleTransform($size / 256.0, $size / 256.0)

    # Hintergrund: abgerundetes Quadrat mit Blauverlauf
    $bg = New-RoundRect 8 8 240 240 48
    $grad = New-Object Drawing.Drawing2D.LinearGradientBrush (New-Object Drawing.PointF 0, 8), (New-Object Drawing.PointF 0, 248),
        ([Drawing.Color]::FromArgb(255, 59, 130, 246)), ([Drawing.Color]::FromArgb(255, 30, 58, 138))
    $g.FillPath($grad, $bg)

    $white = [Drawing.Brushes]::White
    # Monitor: Rahmen, dunkler Bildschirm, Fuß
    $g.FillPath($white, (New-RoundRect 36 52 184 128 16))
    $g.FillPath((New-Object Drawing.SolidBrush ([Drawing.Color]::FromArgb(255, 15, 23, 42))), (New-RoundRect 50 66 156 100 6))
    $g.FillRectangle($white, 114, 178, 28, 22)
    $g.FillPath($white, (New-RoundRect 80 196 96 18 9))

    # Terminal-Prompt ">_" in Cyan
    $cyan = [Drawing.Color]::FromArgb(255, 56, 189, 248)
    $pen = New-Object Drawing.Pen $cyan, 16
    $pen.StartCap = 'Round'; $pen.EndCap = 'Round'; $pen.LineJoin = 'Round'
    $g.DrawLines($pen, [Drawing.PointF[]]@((New-Object Drawing.PointF 76, 90), (New-Object Drawing.PointF 106, 116), (New-Object Drawing.PointF 76, 142)))
    $g.FillPath((New-Object Drawing.SolidBrush $cyan), (New-RoundRect 118 132 52 14 4))
    $g.Dispose()
    $bmp
}

function Get-PngBytes($bmp) {
    $ms = New-Object IO.MemoryStream
    $bmp.Save($ms, [Drawing.Imaging.ImageFormat]::Png)
    $ms.ToArray()
}

# --- ICO mit PNG-Einträgen
$sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
$pngs = foreach ($s in $sizes) { , (Get-PngBytes (Draw-Icon $s)) }
$out = New-Object IO.MemoryStream
$w = New-Object IO.BinaryWriter $out
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $len = $pngs[$i].Length
    $w.Write([byte]($s % 256)); $w.Write([byte]($s % 256))   # 256 → 0
    $w.Write([byte]0); $w.Write([byte]0)                       # Palette, reserviert
    $w.Write([uint16]1); $w.Write([uint16]32)                   # Ebenen, Bit/Pixel
    $w.Write([uint32]$len); $w.Write([uint32]$offset)
    $offset += $len
}
$w.Flush()
foreach ($p in $pngs) { $out.Write([byte[]]$p, 0, $p.Length) }
[IO.File]::WriteAllBytes("$PSScriptRoot\icon.ico", $out.ToArray())

# --- Vorschau
(Draw-Icon 256).Save("$PSScriptRoot\icon_256.png", [Drawing.Imaging.ImageFormat]::Png)

# --- RGBA-Rohdaten 64×64 für das Fenstersymbol
$b = Draw-Icon 64
$rect = New-Object Drawing.Rectangle 0, 0, 64, 64
$data = $b.LockBits($rect, 'ReadOnly', ([Drawing.Imaging.PixelFormat]::Format32bppArgb))
$bgra = New-Object byte[] (64 * 64 * 4)
[Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bgra, 0, $bgra.Length)
$b.UnlockBits($data)
for ($i = 0; $i -lt $bgra.Length; $i += 4) { $t = $bgra[$i]; $bgra[$i] = $bgra[$i + 2]; $bgra[$i + 2] = $t }  # BGRA → RGBA
[IO.File]::WriteAllBytes("$PSScriptRoot\icon_64.rgba", $bgra)
Write-Host "icon.ico, icon_256.png, icon_64.rgba erzeugt"
