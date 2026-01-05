#!/usr/bin/env python3
"""
PDF to Images - Convert PDF pages to PNG images
Usage: python pdf_to_images.py <pdf_path> <output_dir> [dpi]
"""

import sys
import os
from pathlib import Path

def main():
    if len(sys.argv) < 3:
        print("Usage: python pdf_to_images.py <pdf_path> <output_dir> [dpi]", file=sys.stderr)
        sys.exit(1)

    pdf_path = sys.argv[1]
    output_dir = sys.argv[2]
    dpi = int(sys.argv[3]) if len(sys.argv) > 3 else 200

    if not os.path.exists(pdf_path):
        print(f"Error: PDF file not found: {pdf_path}", file=sys.stderr)
        sys.exit(1)

    # Import pdf2image here to provide better error messages
    try:
        from pdf2image import convert_from_path
    except ImportError:
        print("Error: pdf2image not installed. Run: pip install pdf2image", file=sys.stderr)
        sys.exit(1)

    # Create output directory
    Path(output_dir).mkdir(parents=True, exist_ok=True)

    print(f"Converting PDF to images with DPI={dpi}...")

    try:
        images = convert_from_path(pdf_path, dpi=dpi)
    except Exception as e:
        print(f"Error converting PDF: {e}", file=sys.stderr)
        print("Note: Make sure poppler is installed (brew install poppler)", file=sys.stderr)
        sys.exit(1)

    print(f"Total pages: {len(images)}")

    output_paths = []
    for i, image in enumerate(images):
        output_path = os.path.join(output_dir, f"page_{i+1:03d}.png")
        image.save(output_path, "PNG")
        output_paths.append(output_path)
        print(f"  Saved: page_{i+1:03d}.png")

    print(f"\nConversion complete! {len(images)} pages saved to {output_dir}")

    # Output JSON for easy parsing
    import json
    result = {
        "output_dir": output_dir,
        "page_count": len(images),
        "images": output_paths
    }
    print(f"\nJSON_RESULT:{json.dumps(result)}")

if __name__ == "__main__":
    main()
