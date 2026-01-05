#!/usr/bin/env python3
"""
Process PDF - Complete pipeline: PDF -> Images -> Remove Watermark -> PDF
Usage: python process_pdf.py <input_pdf> <output_pdf> [dpi]
"""

import sys
import os
import tempfile
import shutil
from pathlib import Path

def main():
    if len(sys.argv) < 3:
        print("Usage: python process_pdf.py <input_pdf> <output_pdf> [dpi]", file=sys.stderr)
        sys.exit(1)

    input_pdf = sys.argv[1]
    output_pdf = sys.argv[2]
    dpi = int(sys.argv[3]) if len(sys.argv) > 3 else 200

    if not os.path.exists(input_pdf):
        print(f"Error: Input PDF not found: {input_pdf}", file=sys.stderr)
        sys.exit(1)

    # Import required libraries
    try:
        from pdf2image import convert_from_path
        import cv2
        import numpy as np
        import img2pdf
    except ImportError as e:
        print(f"Error: Missing dependency: {e}", file=sys.stderr)
        print("Run: pip install pdf2image opencv-python-headless numpy img2pdf", file=sys.stderr)
        sys.exit(1)

    # Create temporary directory
    temp_dir = tempfile.mkdtemp(prefix="watermark_remover_")
    pages_dir = os.path.join(temp_dir, "pages")
    cleaned_dir = os.path.join(temp_dir, "cleaned")
    os.makedirs(pages_dir)
    os.makedirs(cleaned_dir)

    try:
        # Step 1: Convert PDF to images
        print(f"Step 1/3: Converting PDF to images (DPI={dpi})...")
        try:
            images = convert_from_path(input_pdf, dpi=dpi)
        except Exception as e:
            print(f"Error converting PDF: {e}", file=sys.stderr)
            print("Note: Make sure poppler is installed (brew install poppler)", file=sys.stderr)
            sys.exit(1)

        print(f"  Total pages: {len(images)}")

        page_paths = []
        for i, image in enumerate(images):
            page_path = os.path.join(pages_dir, f"page_{i+1:03d}.png")
            image.save(page_path, "PNG")
            page_paths.append(page_path)

        # Step 2: Remove watermarks
        print(f"\nStep 2/3: Removing watermarks...")
        cleaned_paths = []
        processed_count = 0

        for page_path in page_paths:
            filename = os.path.basename(page_path)
            output_path = os.path.join(cleaned_dir, filename)

            img = cv2.imread(page_path)
            height, width = img.shape[:2]

            # Define watermark region - bottom right corner
            roi_x = int(width * 0.80)
            roi_y = int(height * 0.92)

            # Extract ROI and detect watermark
            roi = img[roi_y:height, roi_x:width]
            gray_roi = cv2.cvtColor(roi, cv2.COLOR_BGR2GRAY)
            mask_roi = cv2.inRange(gray_roi, 150, 240)

            # Morphological operations
            kernel = cv2.getStructuringElement(cv2.MORPH_RECT, (5, 5))
            mask_roi = cv2.dilate(mask_roi, kernel, iterations=2)

            # Create full image mask
            mask = np.zeros((height, width), dtype=np.uint8)
            mask[roi_y:height, roi_x:width] = mask_roi

            # Apply inpainting if watermark detected
            if np.sum(mask) > 100:
                kernel_expand = cv2.getStructuringElement(cv2.MORPH_RECT, (7, 7))
                mask = cv2.dilate(mask, kernel_expand, iterations=1)
                result = cv2.inpaint(img, mask, inpaintRadius=5, flags=cv2.INPAINT_TELEA)
                cv2.imwrite(output_path, result)
                processed_count += 1
                print(f"  {filename}: ✓ Watermark removed")
            else:
                cv2.imwrite(output_path, img)
                print(f"  {filename}: ○ No watermark")

            cleaned_paths.append(output_path)

        # Step 3: Merge images back to PDF
        print(f"\nStep 3/3: Creating output PDF...")

        # Create output directory if needed
        output_dir = os.path.dirname(output_pdf)
        if output_dir:
            Path(output_dir).mkdir(parents=True, exist_ok=True)

        with open(output_pdf, "wb") as f:
            f.write(img2pdf.convert(cleaned_paths))

        # Get file size
        size_bytes = os.path.getsize(output_pdf)
        size_mb = size_bytes / (1024 * 1024)

        print(f"\n{'='*50}")
        print(f"Processing complete!")
        print(f"  Input:  {input_pdf}")
        print(f"  Output: {output_pdf}")
        print(f"  Pages:  {len(images)}")
        print(f"  Watermarks removed: {processed_count}")
        print(f"  File size: {size_mb:.2f} MB")
        print(f"{'='*50}")

        # Output JSON for easy parsing
        import json
        result = {
            "input_pdf": input_pdf,
            "output_pdf": output_pdf,
            "page_count": len(images),
            "watermarks_removed": processed_count,
            "size_bytes": size_bytes
        }
        print(f"JSON_RESULT:{json.dumps(result)}")

    finally:
        # Cleanup temporary directory
        shutil.rmtree(temp_dir, ignore_errors=True)

if __name__ == "__main__":
    main()
