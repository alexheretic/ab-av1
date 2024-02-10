ffmpeg -y -i "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.mkv" -map 0 -c:v copy -c:v:0 libsvtav1 -svtav1-params "scd=0" -crf 32 -pix_fmt yuv420p10le -preset 8 -c:s copy -c:a copy -cues_to_front y "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.av1.mkv"

# ffmpeg -y -i "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.mkv" -map 0 -c:v copy -c:v:0 hevc_videotoolbox -preset main10 -b:v 15000k -pix_fmt p010le -c:s copy -c:a copy -cues_to_front y "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.av1.mkv"

# ffmpeg -y -i "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.mkv" -map 0 -c:v copy -c:v:0 hevc_videotoolbox -profile main10 -q:v 70 -pix_fmt p010le -c:s copy -c:a copy -cues_to_front y "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.av1.mkv"

"ffmpeg" "-y" "-i" "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.mkv" "-map" "0" "-c:v" "copy" "-c:v:0" "libsvtav1" "-svtav1-params" "scd=0" "-preset" "8" "-crf" "32" "-pix_fmt" "yuv420p10le" "-c:s" "copy" "-c:a" "copy" "-cues_to_front" "y" "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.av1.mkv"

"ffmpeg" "-y" "-i" "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.mkv" "-map" "0" "-c:v" "copy" "-c:v:0" "libsvtav1" "-svtav1-params" "scd=0" "-crf" "32" "-pix_fmt" "yuv420p10le" "-preset" "8" "-c:s" "copy" "-c:a" "copy" "-cues_to_front" "y" "/Users/schmatt/Movies/Strange_Brew/.ab-av1-RZu7wZkQ9bDL/Strange Brew_t00.sample585+480f.av1.mkv"

