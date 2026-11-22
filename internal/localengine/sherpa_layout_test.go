package localengine

import (
	"reflect"
	"testing"
)

// cLayout is the size and the field offsets of a C struct of sherpa-onnx
// 1.13.8 (c-api.h) on 64-bit targets, from the #[repr(C)] declarations of
// sherpa-onnx-sys 1.13.8 that the Rust engine links: printed by size_of and
// offset_of on x86_64. Regenerate when the sherpa-onnx version changes.
type cLayout struct {
	size    uintptr
	offsets []uintptr
}

var sherpaLayouts = map[reflect.Type]cLayout{
	reflect.TypeFor[sherpaOfflineTransducerModelConfig]():        {24, []uintptr{0, 8, 16}},                                                                                                                   // OfflineTransducerModelConfig
	reflect.TypeFor[sherpaOfflineParaformerModelConfig]():        {8, []uintptr{0}},                                                                                                                           // OfflineParaformerModelConfig
	reflect.TypeFor[sherpaOfflineNemoEncDecCtcModelConfig]():     {8, []uintptr{0}},                                                                                                                           // OfflineNemoEncDecCtcModelConfig
	reflect.TypeFor[sherpaOfflineWhisperModelConfig]():           {48, []uintptr{0, 8, 16, 24, 32, 36, 40}},                                                                                                   // OfflineWhisperModelConfig
	reflect.TypeFor[sherpaOfflineTdnnModelConfig]():              {8, []uintptr{0}},                                                                                                                           // OfflineTdnnModelConfig
	reflect.TypeFor[sherpaOfflineSenseVoiceModelConfig]():        {24, []uintptr{0, 8, 16}},                                                                                                                   // OfflineSenseVoiceModelConfig
	reflect.TypeFor[sherpaOfflineMoonshineModelConfig]():         {40, []uintptr{0, 8, 16, 24, 32}},                                                                                                           // OfflineMoonshineModelConfig
	reflect.TypeFor[sherpaOfflineFireRedAsrModelConfig]():        {16, []uintptr{0, 8}},                                                                                                                       // OfflineFireRedAsrModelConfig
	reflect.TypeFor[sherpaOfflineDolphinModelConfig]():           {8, []uintptr{0}},                                                                                                                           // OfflineDolphinModelConfig
	reflect.TypeFor[sherpaOfflineZipformerCtcModelConfig]():      {8, []uintptr{0}},                                                                                                                           // OfflineZipformerCtcModelConfig
	reflect.TypeFor[sherpaOfflineCanaryModelConfig]():            {40, []uintptr{0, 8, 16, 24, 32}},                                                                                                           // OfflineCanaryModelConfig
	reflect.TypeFor[sherpaOfflineWenetCtcModelConfig]():          {8, []uintptr{0}},                                                                                                                           // OfflineWenetCtcModelConfig
	reflect.TypeFor[sherpaOfflineOmnilingualAsrCtcModelConfig](): {8, []uintptr{0}},                                                                                                                           // OfflineOmnilingualAsrCtcModelConfig
	reflect.TypeFor[sherpaOfflineMedAsrCtcModelConfig]():         {8, []uintptr{0}},                                                                                                                           // OfflineMedAsrCtcModelConfig
	reflect.TypeFor[sherpaOfflineFunASRNanoModelConfig]():        {88, []uintptr{0, 8, 16, 24, 32, 40, 48, 52, 56, 60, 64, 72, 80}},                                                                           // OfflineFunASRNanoModelConfig
	reflect.TypeFor[sherpaOfflineFireRedAsrCtcModelConfig]():     {8, []uintptr{0}},                                                                                                                           // OfflineFireRedAsrCtcModelConfig
	reflect.TypeFor[sherpaOfflineCohereTranscribeModelConfig]():  {32, []uintptr{0, 8, 16, 24, 28}},                                                                                                           // OfflineCohereTranscribeModelConfig
	reflect.TypeFor[sherpaOfflineModelConfig]():                  {504, []uintptr{0, 24, 32, 40, 88, 96, 104, 108, 112, 120, 128, 136, 144, 152, 176, 216, 232, 240, 248, 288, 296, 304, 312, 400, 408, 472}}, // OfflineModelConfig
	reflect.TypeFor[sherpaFeatureConfig]():                       {8, []uintptr{0, 4}},                                                                                                                        // FeatureConfig
	reflect.TypeFor[sherpaOfflineLMConfig]():                     {16, []uintptr{0, 8}},                                                                                                                       // OfflineLMConfig
	reflect.TypeFor[sherpaHomophoneReplacerConfig]():             {24, []uintptr{0, 8, 16}},                                                                                                                   // HomophoneReplacerConfig
	reflect.TypeFor[sherpaOfflineRecognizerConfig]():             {608, []uintptr{0, 8, 512, 528, 536, 544, 552, 560, 568, 576, 584}},                                                                         // OfflineRecognizerConfig
	reflect.TypeFor[sherpaOnlineTransducerModelConfig]():         {24, []uintptr{0, 8, 16}},                                                                                                                   // OnlineTransducerModelConfig
	reflect.TypeFor[sherpaOnlineParaformerModelConfig]():         {16, []uintptr{0, 8}},                                                                                                                       // OnlineParaformerModelConfig
	reflect.TypeFor[sherpaOnlineZipformer2CtcModelConfig]():      {8, []uintptr{0}},                                                                                                                           // OnlineZipformer2CtcModelConfig
	reflect.TypeFor[sherpaOnlineNemoCtcModelConfig]():            {8, []uintptr{0}},                                                                                                                           // OnlineNemoCtcModelConfig
	reflect.TypeFor[sherpaOnlineToneCtcModelConfig]():            {8, []uintptr{0}},                                                                                                                           // OnlineToneCtcModelConfig
	reflect.TypeFor[sherpaOnlineModelConfig]():                   {136, []uintptr{0, 24, 40, 48, 56, 64, 72, 80, 88, 96, 104, 112, 120, 128}},                                                                 // OnlineModelConfig
	reflect.TypeFor[sherpaOnlineCtcFstDecoderConfig]():           {16, []uintptr{0, 8}},                                                                                                                       // OnlineCtcFstDecoderConfig
	reflect.TypeFor[sherpaOnlineRecognizerConfig]():              {272, []uintptr{0, 8, 144, 152, 156, 160, 164, 168, 176, 184, 192, 208, 216, 224, 232, 240, 248}},                                           // OnlineRecognizerConfig
	reflect.TypeFor[sherpaVadDetectorConfig]():                   {32, []uintptr{0, 8, 12, 16, 20, 24}},                                                                                                       // SileroVadModelConfig
	reflect.TypeFor[sherpaVadModelConfig]():                      {88, []uintptr{0, 32, 36, 40, 48, 56}},                                                                                                      // VadModelConfig
	reflect.TypeFor[sherpaSpeechSegment]():                       {24, []uintptr{0, 8, 16}},                                                                                                                   // SpeechSegment
	reflect.TypeFor[sherpaPyannoteModelConfig]():                 {16, []uintptr{0, 8}},                                                                                                                       // OfflineSpeakerSegmentationPyannoteModelConfig
	reflect.TypeFor[sherpaSegmentationModelConfig]():             {32, []uintptr{0, 16, 20, 24}},                                                                                                              // OfflineSpeakerSegmentationModelConfig
	reflect.TypeFor[sherpaSpeakerEmbeddingExtractorConfig]():     {24, []uintptr{0, 8, 12, 16}},                                                                                                               // SpeakerEmbeddingExtractorConfig
	reflect.TypeFor[sherpaFastClusteringConfig]():                {12, []uintptr{0, 4, 8}},                                                                                                                    // FastClusteringConfig
	reflect.TypeFor[sherpaDiarizationConfig]():                   {80, []uintptr{0, 32, 56, 68, 72}},                                                                                                          // OfflineSpeakerDiarizationConfig
	reflect.TypeFor[sherpaDiarizationSegment]():                  {16, []uintptr{0, 4, 8, 12}},                                                                                                                // OfflineSpeakerDiarizationSegment
}

func TestSherpaStructsMatchTheCLayout(t *testing.T) {
	for typ, want := range sherpaLayouts {
		if typ.Size() != want.size {
			t.Errorf("%s: size %d, C has %d", typ.Name(), typ.Size(), want.size)
		}
		if typ.NumField() != len(want.offsets) {
			t.Errorf("%s: %d fields, C has %d", typ.Name(), typ.NumField(), len(want.offsets))
			continue
		}
		for i, off := range want.offsets {
			if f := typ.Field(i); f.Offset != off {
				t.Errorf("%s.%s: offset %d, C has %d", typ.Name(), f.Name, f.Offset, off)
			}
		}
	}
}
