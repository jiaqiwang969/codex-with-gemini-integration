#!/usr/bin/env python3
"""
Process PDF to Images - Convert PDF to images and remove watermarks
Usage: python process_pdf_to_images.py <input_pdf> <output_dir> [dpi]
"""

import sys
import os
from pathlib import Path

def main():
    if len(sys.argv) < 3:
        print("Usage: python process_pdf_to_images.py <input_pdf> <output_dir> [dpi]", file=sys.stderr)
        sys.exit(1)

    input_pdf = sys.argv[1]
    output_dir = sys.argv[2]
    dpi = int(sys.argv[3]) if len(sys.argv) > 3 else 200

    if not os.path.exists(input_pdf):
        print(f"Error: Input PDF not found: {input_pdf}", file=sys.stderr)
        sys.exit(1)

    # Import required libraries
    try:
        from pdf2image import convert_from_path
        import cv2
        import numpy as np
    except ImportError as e:
        print(f"Error: Missing dependency: {e}", file=sys.stderr)
        print("Run: pip install pdf2image opencv-python-headless numpy", file=sys.stderr)
        sys.exit(1)

    # Create output directory
    Path(output_dir).mkdir(parents=True, exist_ok=True)

    # Step 1: Convert PDF to images
    print(f"Step 1/2: Converting PDF to images (DPI={dpi})...")
    try:
        images = convert_from_path(input_pdf, dpi=dpi)
    except Exception as e:
        print(f"Error converting PDF: {e}", file=sys.stderr)
        print("Note: Make sure poppler is installed (brew install poppler)", file=sys.stderr)
        sys.exit(1)

    print(f"  Total pages: {len(images)}")

    # Step 2: Remove watermarks and save
    print(f"\nStep 2/2: Removing watermarks and saving...")
    processed_count = 0

    for i, image in enumerate(images):
        # Save temporarily to process with OpenCV
        temp_path = os.path.join(output_dir, f"_temp_{i}.png")
        output_path = os.path.join(output_dir, f"page_{i+1:03d}.png")

        image.save(temp_path, "PNG")

        # Load with OpenCV
        img = cv2.imread(temp_path)
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
            print(f"  page_{i+1:03d}.png: ✓ Watermark removed")
        else:
            cv2.imwrite(output_path, img)
            print(f"  page_{i+1:03d}.png: ○ No watermark")

        # Remove temp file
        os.remove(temp_path)

    print(f"\n{'='*50}")
    print(f"Processing complete!")
    print(f"  Input:  {input_pdf}")
    print(f"  Output: {output_dir}")
    print(f"  Pages:  {len(images)}")
    print(f"  Watermarks removed: {processed_count}")
    print(f"{'='*50}")

if __name__ == "__main__":
    main()
