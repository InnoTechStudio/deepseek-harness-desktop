# 生成 Windows 安装程序的品牌图（NSIS MUI2 头图与侧边图）。
#
# NSIS 对 BMP 的要求很挑：DIB 头必须是 12 或 40 字节、必须自底向上存储。
# macOS 的 `sips -s format bmp` 写出的是自顶向下（biHeight 为负），NSIS 会
# 直接拒绝，而 Tauri 不校验图片、makensis 只报 5040 警告——构建照样成功，
# 但装出来的包用的是 NSIS 默认灰底图。所以这里用 Pillow 生成，并在最后
# 逐字节校验 DIB 头与高度符号。
#
# 用法：python3 scripts/make-installer-art.py

import struct
import sys
from pathlib import Path

try:
    from PIL import Image, ImageDraw
except ImportError:
    sys.exit("需要 Pillow：pip3 install pillow")

ROOT = Path(__file__).resolve().parent.parent
ICON = ROOT / "src-tauri/icons/512x512.png"
OUT_DIR = ROOT / "src-tauri/installer"

# MUI2 的推荐尺寸。尺寸不符不会报错但会被拉伸，视觉上很难看。
HEADER_SIZE = (150, 57)
SIDEBAR_SIZE = (164, 314)

# 取自应用图标的中性色调：接近白的底 + 近黑的前景，和客户端浅色主题一致。
PAPER = (250, 250, 251)
INK = (24, 24, 27)
MUTED = (113, 113, 122)
HAIRLINE = (228, 228, 231)


def load_logo(box: int) -> Image.Image:
    """把方形应用图标缩放到 box×box，保留透明通道。"""
    logo = Image.open(ICON).convert("RGBA")
    return logo.resize((box, box), Image.LANCZOS)


def paste_logo(canvas: Image.Image, logo: Image.Image, xy: tuple[int, int]) -> None:
    """按 alpha 合成。BMP 不支持透明，所以必须贴到不透明底上。"""
    canvas.paste(logo, xy, logo)


def build_header() -> Image.Image:
    """头图：出现在除欢迎/结束页之外每一页的右上角，很小，只放图标。"""
    canvas = Image.new("RGB", HEADER_SIZE, PAPER)
    box = HEADER_SIZE[1] - 14
    logo = load_logo(box)
    # 靠右留 12px 边距，垂直居中。
    paste_logo(canvas, logo, (HEADER_SIZE[0] - box - 12, (HEADER_SIZE[1] - box) // 2))
    draw = ImageDraw.Draw(canvas)
    # 底部发丝线，和安装页正文区分开。
    draw.line([(0, HEADER_SIZE[1] - 1), (HEADER_SIZE[0], HEADER_SIZE[1] - 1)], fill=HAIRLINE)
    return canvas


def build_sidebar() -> Image.Image:
    """侧边图：欢迎页与结束页左侧整条，放大图标 + 产品名。"""
    canvas = Image.new("RGB", SIDEBAR_SIZE, PAPER)
    draw = ImageDraw.Draw(canvas)

    box = 96
    logo = load_logo(box)
    paste_logo(canvas, logo, ((SIDEBAR_SIZE[0] - box) // 2, 74))

    # 产品名分两行居中。位图字体够小，用默认字体避免依赖系统字体文件。
    for index, (text, fill) in enumerate([("DeepSeek", INK), ("Harness", MUTED)]):
        width = draw.textlength(text)
        draw.text(((SIDEBAR_SIZE[0] - width) / 2, 188 + index * 15), text, fill=fill)

    # 右侧发丝线，与安装页正文衔接。
    draw.line([(SIDEBAR_SIZE[0] - 1, 0), (SIDEBAR_SIZE[0] - 1, SIDEBAR_SIZE[1])], fill=HAIRLINE)
    return canvas


def verify(path: Path, expected: tuple[int, int]) -> None:
    """校验 NSIS 能否加载：DIB 头 12/40 字节、biHeight 为正、尺寸正确。"""
    data = path.read_bytes()
    if data[:2] != b"BM":
        raise SystemExit(f"{path.name}: 不是 BMP")
    header_size = struct.unpack_from("<I", data, 14)[0]
    if header_size not in (12, 40):
        raise SystemExit(f"{path.name}: DIB 头 {header_size} 字节，NSIS 只接受 12 或 40")
    width, height = struct.unpack_from("<ii", data, 18)
    if height <= 0:
        raise SystemExit(f"{path.name}: 自顶向下存储（高度 {height}），NSIS 会拒绝")
    if (width, height) != expected:
        raise SystemExit(f"{path.name}: 尺寸 {width}x{height}，应为 {expected[0]}x{expected[1]}")
    print(f"  {path.name}  {width}x{height}  DIB {header_size}B  自底向上  {len(data):,} 字节")


def main() -> None:
    if not ICON.exists():
        raise SystemExit(f"找不到图标：{ICON}")
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    targets = [
        (OUT_DIR / "header.bmp", build_header(), HEADER_SIZE),
        (OUT_DIR / "sidebar.bmp", build_sidebar(), SIDEBAR_SIZE),
    ]
    print("生成安装程序品牌图：")
    for path, image, expected in targets:
        # BMP3 强制 40 字节 BITMAPINFOHEADER，避免写成 V4/V5 头。
        image.save(path, format="BMP")
        verify(path, expected)
    print(f"\n输出目录：{OUT_DIR.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
