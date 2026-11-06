#!/bin/sh
# Download the source data into data/ (about 1 GB; resumes partial downloads):
#   PIA19952        NASA/JHUAPL/SwRI enhanced-colour portrait of Pluto, 5000x5000
#   lorri.tif       USGS Pluto New Horizons LORRI/MVIC global mosaic, 300 m/px (8 bit)
#   dem.tif         USGS Pluto New Horizons LORRI/MVIC global DEM, 300 m/px (int16 metres)
set -e
cd "$(dirname "$0")/.."
mkdir -p data/pia
get() { # url file size
  while [ "$(stat -c %s "$2" 2>/dev/null || echo 0)" -lt "$3" ]; do
    curl -fL -C - -o "$2" "$1" || sleep 2
  done
}
get https://assets.science.nasa.gov/content/dam/science/psd/photojournal/pia/pia19/pia19952/PIA19952.jpg \
    data/pia/PIA19952.jpg 1
get https://planetarymaps.usgs.gov/mosaic/Pluto_NewHorizons_Global_Mosaic_300m_Jul2017_8bit.tif \
    data/lorri.tif 309806463
get https://planetarymaps.usgs.gov/mosaic/Pluto_NewHorizons_Global_DEM_300m_Jul2017_16bit.tif \
    data/dem.tif 619512743
echo "data ready: python3 tools/fit_photo.py && python3 tools/build_view.py"
