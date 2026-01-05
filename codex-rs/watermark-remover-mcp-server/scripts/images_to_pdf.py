#!/usr/bin/env python3
"""
Images to PDF - Merge images into a PDF file
Usage: python images_to_pdf.py <image_dir> <output_path> [pattern]
"""

import sys
import os
from pathlib import Path
import glob

def main():
    if len(sys.argv) < 3:
        print("Usage: python images_to_pdf.py <image_dir> <output_path> [pattern]", file=sys.stderr)
        sys.exit(1)

    image_dir = sys.argv[1]
    output_path = sys.argv[2]
    pattern = sys.argv[3] if len(sys.argv) > 3 else "*.png"

    if not os.path.isdir(image_dir):
        print(f"Error: Directory not found: {image_dir}", file=sys.stderr)
        sys.exit(1)

    # Import img2pdf here to provide better error messages
    try:
        import img2pdf
    except ImportError:
        print("Error: img2pdf not installed. Run: pip install img2pdf", file=sys.stderr)
        sys.exit(1)

    # Find all matching images
    search_pattern = os.path.join(image_dir, pattern)
    image_files = sorted(glob.glob(search_pattern))

    if not image_files:
        # Try without pattern, just get all images
        image_extensions = ['*.png', '*.jpg', '*.jpeg', '*.webp']
        for ext in image_extensions:
            image_files.extend(glob.glob(os.path.join(image_dir, ext)))
        image_files = sorted(set(image_files))

    if not image_files:
        print(f"Error: No images found in {image_dir}", file=sys.stderr)
        sys.exit(1)

    print(f"Found {len(image_files)} images")
    for f in image_files:
        print(f"  - {os.path.basename(f)}")

    # Create output directory if needed
    output_dir = os.path.dirname(output_path)
    if output_dir:
        Path(output_dir).mkdir(parents=True, exist_ok=True)

    # Merge images to PDF
    print(f"\nMerging to PDF: {output_path}")

    try:
        with open(output_path, "wb") as f:
            f.write(img2pdf.convert(image_files))
    except Exception as e:
        print(f"Error creating PDF: {e}", file=sys.stderr)
        sys.exit(1)

    # Get file size
    size_bytes = os.path.getsize(output_path)
    size_mb = size_bytes / (1024 * 1024)

    print(f"\nPDF created successfully!")
    print(f"  Output: {output_path}")
    print(f"  Size: {size_mb:.2f} MB")
    print(f"  Pages: {len(image_files)}")

    # Output JSON for easy parsing
    import json
    result = {
        "output_path": output_path,
        "page_count": len(image_files),
        "size_bytes": size_bytes
    }
    print(f"JSON_RESULT:{json.dumps(result)}")

if __name__ == "__main__":
    main()
