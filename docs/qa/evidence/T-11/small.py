import sys, xml.etree.ElementTree as ET
dp = 3.0  # 480 dpi
for n in ET.parse(sys.argv[1]).iter("node"):
    if n.get("clickable") != "true": continue
    x1,y1,x2,y2 = [int(v) for v in n.get("bounds").replace("][",",").strip("[]").split(",")]
    w,h = (x2-x1)/dp, (y2-y1)/dp
    if w < 48 or h < 48:
        print(f"{w:.0f}x{h:.0f}dp  {n.get('text') or n.get('content-desc') or n.get('class')}")
