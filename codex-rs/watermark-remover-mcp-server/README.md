# Watermark Remover MCP Server

A Model Context Protocol (MCP) server for removing watermarks from PDF files and images.

## Features

- **PDF to Images**: Convert PDF pages to high-quality PNG images
- **Remove Watermark**: Remove watermarks (like NotebookLM) from images using OpenCV
- **Images to PDF**: Merge images back into a PDF file
- **Process PDF**: One-click pipeline for complete watermark removal

## Installation

### Prerequisites

1. **Python 3.10+** with pip
2. **poppler** for PDF processing:
   ```bash
   # macOS
   brew install poppler

   # Ubuntu/Debian
   sudo apt install poppler-utils
   ```

### Install Python Dependencies

```bash
cd watermark-remover-mcp-server/scripts
pip install -r requirements.txt
```

### Build the Server

```bash
cargo build --release -p watermark-remover-mcp-server
```

## Usage

### As MCP Server

Add to your MCP configuration:

```json
{
  "mcpServers": {
    "watermark-remover": {
      "command": "/path/to/watermark-remover-mcp-server",
      "env": {
        "WATERMARK_SCRIPTS_DIR": "/path/to/scripts"
      }
    }
  }
}
```

### Available Tools

#### `pdf_to_images`
Convert PDF to PNG images.

```json
{
  "pdf_path": "/path/to/input.pdf",
  "output_dir": "/path/to/output",
  "dpi": 200
}
```

#### `remove_watermark`
Remove watermarks from images.

```json
{
  "image_dir": "/path/to/images",
  "output_dir": "/path/to/output"
}
```

#### `images_to_pdf`
Merge images into PDF.

```json
{
  "image_dir": "/path/to/images",
  "output_path": "/path/to/output.pdf"
}
```

#### `process_pdf`
Complete pipeline: PDF → Remove Watermark → PDF.

```json
{
  "pdf_path": "/path/to/input.pdf",
  "output_path": "/path/to/output.pdf",
  "dpi": 200
}
```

## How It Works

The watermark removal algorithm:

1. Detects the bottom-right corner region (where NotebookLM watermarks typically appear)
2. Identifies light-colored text using grayscale thresholding (150-240 range)
3. Applies morphological dilation to connect text fragments
4. Uses OpenCV's `inpaint` function (Telea algorithm) to seamlessly fill the watermark area

## License

MIT
