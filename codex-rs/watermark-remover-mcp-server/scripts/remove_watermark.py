#!/usr/bin/env python3
"""
Remove Watermark - Remove watermarks from images using OpenCV
Usage: python remove_watermark.py --image <path> | --dir <path> [--output <dir>]
"""

import sys
import os
import argparse
from pathlib import Path

def remove_watermark(image_path, output_path):
    """Remove watermark from a single image."""
    import cv2
    import numpy as np

    img = cv2.imread(image_path)
    if img is None:
        print(f"  Error: Cannot read image: {image_path}", file=sys.stderr)
        return False

    height, width = img.shape[:2]

    # Define watermark region - bottom right corner
    # NotebookLM watermark is typically in the bottom-right 20% x 8% area
    roi_x = int(width * 0.80)
    roi_y = int(height * 0.92)
    roi_w = width - roi_x
    roi_h = height - roi_y

    # Extract ROI
    roi = img[roi_y:height, roi_x:width]

    # Convert to grayscale
    gray_roi = cv2.cvtColor(roi, cv2.COLOR_BGR2GRAY)

    # Detect light-colored text (watermarks are usually light gray)
    # Watermark text color is approximately in the 150-240 range
    mask_roi = cv2.inRange(gray_roi, 150, 240)

    # Use morphological operations to connect watermark text parts
    kernel = cv2.getStructuringElement(cv2.MORPH_RECT, (5, 5))
    mask_roi = cv2.dilate(mask_roi, kernel, iterations=2)

    # Create full image mask
    mask = np.zeros((height, width), dtype=np.uint8)
    mask[roi_y:height, roi_x:width] = mask_roi

    # Check if watermark was detected
    if np.sum(mask) > 100:
        # Expand mask to ensure full coverage
        kernel_expand = cv2.getStructuringElement(cv2.MORPH_RECT, (7, 7))
        mask = cv2.dilate(mask, kernel_expand, iterations=1)

        # Use OpenCV inpaint to repair
        result = cv2.inpaint(img, mask, inpaintRadius=5, flags=cv2.INPAINT_TELEA)

        cv2.imwrite(output_path, result)
        return True
    else:
        # No watermark detected, copy original
        cv2.imwrite(output_path, img)
        return False

def main():
    parser = argparse.ArgumentParser(description='Remove watermarks from images')
    parser.add_argument('--image', help='Single image path')
    parser.add_argument('--dir', help='Directory containing images')
    parser.add_argument('--output', help='Output directory (optional)')

    args = parser.parse_args()

    if not args.image and not args.dir:
        print("Error: Either --image or --dir must be provided", file=sys.stderr)
        sys.exit(1)

    # Import OpenCV here to provide better error messages
    try:
        import cv2
        import numpy as np
    except ImportError:
        print("Error: opencv-python not installed. Run: pip install opencv-python-headless numpy", file=sys.stderr)
        sys.exit(1)

    processed_count = 0
    skipped_count = 0

    if args.image:
        # Process single image
        image_path = args.image
        if not os.path.exists(image_path):
            print(f"Error: Image not found: {image_path}", file=sys.stderr)
            sys.exit(1)

        if args.output:
            Path(args.output).mkdir(parents=True, exist_ok=True)
            output_path = os.path.join(args.output, os.path.basename(image_path))
        else:
            # Overwrite original
            output_path = image_path

        print(f"Processing: {image_path}")
        if remove_watermark(image_path, output_path):
            print(f"  ✓ Watermark removed: {output_path}")
            processed_count = 1
        else:
            print(f"  ○ No watermark detected: {output_path}")
            skipped_count = 1

    elif args.dir:
        # Process directory
        image_dir = args.dir
        if not os.path.isdir(image_dir):
            print(f"Error: Directory not found: {image_dir}", file=sys.stderr)
            sys.exit(1)

        output_dir = args.output if args.output else image_dir
        Path(output_dir).mkdir(parents=True, exist_ok=True)

        # Get all image files
        image_extensions = {'.png', '.jpg', '.jpeg', '.webp', '.gif'}
        image_files = sorted([
            f for f in os.listdir(image_dir)
            if os.path.isfile(os.path.join(image_dir, f))
            and Path(f).suffix.lower() in image_extensions
            and not f.endswith('_processed.png')  # Skip already processed
        ])

        print(f"Found {len(image_files)} images in {image_dir}")

        for image_file in image_files:
            input_path = os.path.join(image_dir, image_file)
            output_path = os.path.join(output_dir, image_file)

            print(f"Processing: {image_file}")
            if remove_watermark(input_path, output_path):
                print(f"  ✓ Watermark removed")
                processed_count += 1
            else:
                print(f"  ○ No watermark detected")
                skipped_count += 1

    print(f"\nComplete! Processed: {processed_count}, Skipped: {skipped_count}")

    # Output JSON for easy parsing
    import json
    result = {
        "processed": processed_count,
        "skipped": skipped_count,
        "output_dir": args.output or (args.dir if args.dir else os.path.dirname(args.image))
    }
    print(f"JSON_RESULT:{json.dumps(result)}")

if __name__ == "__main__":
    main()
