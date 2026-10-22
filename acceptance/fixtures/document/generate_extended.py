"""Generate owned ODF/EPUB/RTF and image boundary fixtures without Office.

Legacy DOC/XLS/PPT can be regenerated with LibreOffice headless from the existing
heading-table.docx, sheets-formula.xlsx and slides-notes.pptx fixtures. Never rename
an OOXML file to a legacy extension as a substitute for format validation.
"""
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED, ZIP_STORED
import struct
import zlib

from PIL import Image
import pymupdf as fitz

ROOT = Path(__file__).resolve().parent


def package(name, mime, content):
    with ZipFile(ROOT / name, "w", ZIP_DEFLATED) as archive:
        archive.writestr("mimetype", mime, compress_type=ZIP_STORED)
        archive.writestr("content.xml", content)
        archive.writestr("META-INF/manifest.xml", f'''<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2">
<manifest:file-entry manifest:full-path="/" manifest:media-type="{mime}"/>
<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>
</manifest:manifest>''')


def main():
    prefix = '''<?xml version="1.0" encoding="UTF-8"?><office:document-content office:version="1.2"
 xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
 xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
 xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0"
 xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"
 xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2"><office:body>'''
    suffix = '</office:body></office:document-content>'
    package('extended.odt', 'application/vnd.oasis.opendocument.text', prefix + '''<office:text>
<text:h text:outline-level="1">中文风险评估</text:h><text:p>付款期限 42 天。ODF evidence.</text:p>
</office:text>''' + suffix)
    package('extended.ods', 'application/vnd.oasis.opendocument.spreadsheet', prefix + '''<office:spreadsheet>
<table:table table:name="风险"><table:table-row><table:table-cell office:value-type="string"><text:p>项目</text:p></table:table-cell>
<table:table-cell office:value-type="float" office:value="42"><text:p>42</text:p></table:table-cell>
<table:table-cell table:formula="of:=[.B1]*2" office:value-type="float" office:value="84"><text:p>84</text:p></table:table-cell></table:table-row></table:table>
</office:spreadsheet>''' + suffix)
    package('extended.odp', 'application/vnd.oasis.opendocument.presentation', prefix + '''<office:presentation>
<draw:page draw:name="第一页"><draw:frame svg:x="1cm" svg:y="1cm" svg:width="15cm" svg:height="5cm"><draw:text-box><text:p>中文风险汇报 42</text:p></draw:text-box></draw:frame></draw:page>
<draw:page draw:name="第二页"><draw:frame svg:x="1cm" svg:y="1cm" svg:width="15cm" svg:height="5cm"><draw:text-box><text:p>Summary 81</text:p></draw:text-box></draw:frame></draw:page>
</office:presentation>''' + suffix)
    with ZipFile(ROOT / 'extended.epub', 'w', ZIP_DEFLATED) as archive:
        archive.writestr('mimetype', 'application/epub+zip', compress_type=ZIP_STORED)
        archive.writestr('META-INF/container.xml', '''<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>''')
        archive.writestr('book.opf', '''<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="id">pixels-owned-fixture</dc:identifier><dc:title>中文验收</dc:title><dc:language>zh</dc:language></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx"><itemref idref="chapter"/></spine></package>''')
        archive.writestr('toc.ncx', '''<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head><meta name="dtb:uid" content="pixels-owned-fixture"/><meta name="dtb:depth" content="1"/><meta name="dtb:totalPageCount" content="0"/><meta name="dtb:maxPageNumber" content="0"/></head><docTitle><text>中文验收</text></docTitle><navMap><navPoint id="chapter" playOrder="1"><navLabel><text>风险</text></navLabel><content src="chapter.xhtml"/></navPoint></navMap></ncx>''')
        archive.writestr('chapter.xhtml', '''<html xmlns="http://www.w3.org/1999/xhtml"><head><title>风险</title></head><body><h1>中文风险条款</h1><p>EPUB payment 42 days.</p></body></html>''')
    text = '中文风险条款 RTF payment 42 days.'
    escaped = ''.join(f'\\u{ord(c) if ord(c) < 32768 else ord(c)-65536}?' if ord(c)>127 else c for c in text)
    (ROOT / 'extended.rtf').write_bytes(('{\\rtf1\\ansi\\ansicpg1252\\uc1 ' + escaped + '\\par}').encode('ascii'))
    with Image.open(ROOT / 'scan-zh-en.png') as image:
        image.convert('RGB').save(ROOT / 'scan-zh-en.jpg', quality=95)
        rotated = image.transpose(Image.Transpose.ROTATE_90)
        exif = rotated.getexif(); exif[274] = 6
        rotated.save(ROOT / 'scan-exif.jpg', exif=exif, quality=95)
    Image.new('RGB', (600, 400), 'white').save(ROOT / 'blank.png')
    # Valid PNG header advertising a huge raster; reject before allocation.
    def chunk(kind, data):
        return struct.pack('>I',len(data)) + kind + data + struct.pack('>I',zlib.crc32(kind+data))
    (ROOT / 'oversized-raster.png').write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR',struct.pack('>IIBBBBB',32768,32768,8,2,0,0,0)) + chunk(b'IDAT',zlib.compress(b'\0'*16)) + chunk(b'IEND',b''))
    (ROOT / 'malformed.png').write_bytes(b'not an image')
    with fitz.open() as pdf:
        image_ref = 0
        for _ in range(256):
            page = pdf.new_page(width=750, height=210)
            image_ref = page.insert_image(page.rect, filename=str(ROOT / 'scan-zh-en.png'), xref=image_ref)
        pdf.save(ROOT / 'ocr-cancel.pdf', deflate=True)


if __name__ == '__main__':
    main()
