package app.sayso.mobile.asr

/**
 * Cuts [samples] into pieces of at most [maxLen] samples, for a model that
 * cannot decode a long recording in one pass. Each cut is at the quietest
 * [frame] in the last [search] samples before the limit, so it falls in a
 * pause between words when there is one. A recording within [maxLen] is one
 * piece.
 */
fun splitAtPauses(samples: FloatArray, maxLen: Int, search: Int, frame: Int): List<IntRange> {
    require(maxLen > 0 && frame in 1..maxLen && search in frame..maxLen)
    val pieces = mutableListOf<IntRange>()
    var start = 0
    while (samples.size - start > maxLen) {
        val limit = start + maxLen
        var cut = limit
        var quietest = Double.MAX_VALUE
        var f = limit - search
        while (f + frame <= limit) {
            var energy = 0.0
            for (i in f until f + frame) energy += samples[i] * samples[i]
            if (energy < quietest) {
                quietest = energy
                cut = maxOf(f + frame / 2, start + 1)
            }
            f += maxOf(1, frame / 2)
        }
        pieces += start until cut
        start = cut
    }
    pieces += start until samples.size
    return pieces
}
