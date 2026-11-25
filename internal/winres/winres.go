// Package winres makes the Windows resources of Sayso.exe and checks a built
// exe for them.
//
// The Windows backend of MyGo gives its window class the icon resource 1
// (LoadIconW(instance, 1)). mygo build links one in; go build links the .syso
// files of the main package, so cmd/sayso keeps rsrc_windows_<arch>.syso,
// made by Generate from assets/app/AppIcon.ico (go run ./scripts/winres
// generate). Without them the window, the taskbar, and Alt+Tab show the
// default icon.
package winres

import (
	"bytes"
	"debug/pe"
	"encoding/binary"
	"errors"
	"fmt"
	"image"
	"image/png"
	"os"
	"slices"

	"golang.org/x/image/draw"
)

const (
	rtIcon      = 3
	rtGroupIcon = 14
	// groupID is the id that MyGo loads.
	groupID = 1
)

// IconSizes are the sizes the shell asks for at 100 % to 200 % scale: 16, 20,
// 24 for the small icon, 32, 40, 48 for the big one and the taskbar, and
// 64 to 256 for Explorer. A size the .ico lacks is drawn from its largest.
var IconSizes = []int{16, 20, 24, 32, 40, 48, 64, 128, 256}

// arches are the COFF machines and the relocation type of an image-relative
// address (ADDR32NB) on each.
var arches = []struct {
	goarch              string
	machine, relocation uint16
}{
	{"amd64", 0x8664, 0x0003},
	{"arm64", 0xAA64, 0x0002},
}

// Generate returns the .syso files, by name, for the icon file.
func Generate(icoPath string) (map[string][]byte, error) {
	data, err := os.ReadFile(icoPath)
	if err != nil {
		return nil, err
	}
	images, err := iconImages(data)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", icoPath, err)
	}
	out := map[string][]byte{}
	for _, a := range arches {
		out["rsrc_windows_"+a.goarch+".syso"] = syso(images, a.machine, a.relocation)
	}
	return out, nil
}

// iconImages returns a PNG for each of IconSizes: the entry of the .ico with
// that size, else the largest entry scaled down.
func iconImages(ico []byte) ([][]byte, error) {
	entries, err := readICO(ico)
	if err != nil {
		return nil, err
	}
	largest := 0
	for s := range entries {
		largest = max(largest, s)
	}
	if largest < IconSizes[len(IconSizes)-1] {
		return nil, fmt.Errorf("the largest image is %d px; it needs %d", largest, IconSizes[len(IconSizes)-1])
	}
	src, err := png.Decode(bytes.NewReader(entries[largest]))
	if err != nil {
		return nil, err
	}
	out := make([][]byte, len(IconSizes))
	for i, s := range IconSizes {
		if b, ok := entries[s]; ok {
			out[i] = b
			continue
		}
		dst := image.NewNRGBA(image.Rect(0, 0, s, s))
		draw.CatmullRom.Scale(dst, dst.Bounds(), src, src.Bounds(), draw.Src, nil)
		var buf bytes.Buffer
		if err := png.Encode(&buf, dst); err != nil {
			return nil, err
		}
		out[i] = buf.Bytes()
	}
	return out, nil
}

// readICO returns the PNG entries of an .ico by their size.
func readICO(b []byte) (map[int][]byte, error) {
	if len(b) < 6 || binary.LittleEndian.Uint16(b[0:]) != 0 || binary.LittleEndian.Uint16(b[2:]) != 1 {
		return nil, errors.New("not an .ico file")
	}
	n := int(binary.LittleEndian.Uint16(b[4:]))
	out := map[int][]byte{}
	for i := range n {
		e := 6 + 16*i
		if e+16 > len(b) {
			return nil, errors.New("the directory is cut short")
		}
		size := int(b[e])
		if size == 0 {
			size = 256
		}
		length, offset := binary.LittleEndian.Uint32(b[e+8:]), binary.LittleEndian.Uint32(b[e+12:])
		if uint64(offset)+uint64(length) > uint64(len(b)) {
			return nil, fmt.Errorf("the %d px image is cut short", size)
		}
		img := b[offset : offset+length]
		if !bytes.HasPrefix(img, []byte("\x89PNG")) {
			return nil, fmt.Errorf("the %d px image is not a PNG", size)
		}
		cfg, err := png.DecodeConfig(bytes.NewReader(img))
		if err != nil || cfg.Width != size || cfg.Height != size {
			return nil, fmt.Errorf("the %d px entry does not hold a %d px PNG", size, size)
		}
		out[size] = img
	}
	return out, nil
}

// syso builds a COFF object with one .rsrc section: the icon images as
// RT_ICON 1 to n and RT_GROUP_ICON 1 that lists them, language neutral. The
// section has three levels of directories (type, id, language), the data
// entries, then the data; data entries hold image-relative addresses, which
// the linker fills in from the relocations.
func syso(images [][]byte, machine, relocation uint16) []byte {
	le := binary.LittleEndian
	var group bytes.Buffer
	_ = binary.Write(&group, le, [3]uint16{0, 1, uint16(len(images))})
	icons := map[uint16][]byte{}
	for i, img := range images {
		id := uint16(i + 1)
		icons[id] = img
		dim := byte(IconSizes[i])
		if IconSizes[i] >= 256 {
			dim = 0
		}
		group.Write([]byte{dim, dim, 0, 0})
		_ = binary.Write(&group, le, [2]uint16{1, 32})
		_ = binary.Write(&group, le, uint32(len(img)))
		_ = binary.Write(&group, le, id)
	}
	res := map[uint16]map[uint16][]byte{
		rtIcon:      icons,
		rtGroupIcon: {groupID: group.Bytes()},
	}

	types := sortedKeys(res)
	dirSize := func(n int) int { return 16 + 8*n }
	size := dirSize(len(types))
	count := 0
	for _, t := range types {
		size += dirSize(len(res[t])) + len(res[t])*dirSize(1)
		count += len(res[t])
	}
	entriesAt := size
	dataAt := entriesAt + 16*count

	raw := make([]byte, dataAt)
	var data bytes.Buffer
	var relocs []uint32
	le.PutUint16(raw[14:], uint16(len(types)))
	next := dirSize(len(types))
	entry := entriesAt
	for i, t := range types {
		le.PutUint32(raw[16+8*i:], uint32(t))
		le.PutUint32(raw[16+8*i+4:], uint32(next)|0x80000000)
		ids := sortedKeys(res[t])
		idDir := next
		le.PutUint16(raw[idDir+14:], uint16(len(ids)))
		next += dirSize(len(ids))
		for j, id := range ids {
			le.PutUint32(raw[idDir+16+8*j:], uint32(id))
			le.PutUint32(raw[idDir+16+8*j+4:], uint32(next)|0x80000000)
			le.PutUint16(raw[next+14:], 1)
			le.PutUint32(raw[next+16:], 0) // LANG_NEUTRAL
			le.PutUint32(raw[next+20:], uint32(entry))
			next += dirSize(1)
			for data.Len()%8 != 0 {
				data.WriteByte(0)
			}
			le.PutUint32(raw[entry:], uint32(dataAt+data.Len()))
			le.PutUint32(raw[entry+4:], uint32(len(res[t][id])))
			relocs = append(relocs, uint32(entry))
			entry += 16
			data.Write(res[t][id])
		}
	}
	raw = append(raw, data.Bytes()...)
	for len(raw)%4 != 0 {
		raw = append(raw, 0)
	}

	var out bytes.Buffer
	const headerSize, sectionHeaderSize = 20, 40
	rawAt := headerSize + sectionHeaderSize
	relocAt := rawAt + len(raw)
	symbolsAt := relocAt + 10*len(relocs)
	_ = binary.Write(&out, le, struct {
		Machine, Sections                   uint16
		Time, Symbols, SymbolCount          uint32
		OptionalHeaderSize, Characteristics uint16
	}{machine, 1, 0, uint32(symbolsAt), 1, 0, 0})
	_ = binary.Write(&out, le, struct {
		Name                                        [8]byte
		VirtualSize, VirtualAddress, RawSize, RawAt uint32
		RelocAt, LineNumbersAt                      uint32
		Relocs, LineNumbers                         uint16
		Characteristics                             uint32
	}{[8]byte{'.', 'r', 's', 'r', 'c'}, 0, 0, uint32(len(raw)), uint32(rawAt), uint32(relocAt), 0, uint16(len(relocs)), 0, 0x40000040})
	out.Write(raw)
	for _, r := range relocs {
		_ = binary.Write(&out, le, r)
		_ = binary.Write(&out, le, uint32(0)) // symbol 0: the section
		_ = binary.Write(&out, le, relocation)
	}
	out.Write([]byte{'.', 'r', 's', 'r', 'c', 0, 0, 0})
	_ = binary.Write(&out, le, uint32(0)) // Value
	_ = binary.Write(&out, le, uint16(1)) // SectionNumber
	_ = binary.Write(&out, le, uint16(0)) // Type
	out.Write([]byte{3, 0})               // IMAGE_SYM_CLASS_STATIC, no aux symbols
	_ = binary.Write(&out, le, uint32(4)) // an empty string table
	return out.Bytes()
}

func sortedKeys[V any](m map[uint16]V) []uint16 {
	keys := make([]uint16, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	return keys
}

// Check reads the resource section of a PE file and returns the sizes of
// the images of icon group 1. It fails when the group is missing, an image
// it lists is missing, or a size of IconSizes is not there.
func Check(path string) ([]int, error) {
	f, err := pe.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	sec := f.Section(".rsrc")
	if sec == nil {
		return nil, errors.New("no .rsrc section: the exe has no resources (was cmd/sayso built without its .syso files?)")
	}
	raw, err := sec.Data()
	if err != nil {
		return nil, err
	}
	r := rsrc{raw: raw, base: sec.VirtualAddress}
	group, err := r.lookup(rtGroupIcon, groupID)
	if err != nil {
		return nil, fmt.Errorf("RT_GROUP_ICON %d: %w", groupID, err)
	}
	if len(group) < 6 {
		return nil, errors.New("the icon group is cut short")
	}
	n := int(binary.LittleEndian.Uint16(group[4:]))
	var sizes []int
	for i := range n {
		e := 6 + 14*i
		if e+14 > len(group) {
			return nil, errors.New("the icon group is cut short")
		}
		s := int(group[e])
		if s == 0 {
			s = 256
		}
		id := binary.LittleEndian.Uint16(group[e+12:])
		img, err := r.lookup(rtIcon, id)
		if err != nil {
			return nil, fmt.Errorf("RT_ICON %d (%d px): %w", id, s, err)
		}
		if uint32(len(img)) != binary.LittleEndian.Uint32(group[e+8:]) {
			return nil, fmt.Errorf("RT_ICON %d has %d bytes, the group says %d", id, len(img), binary.LittleEndian.Uint32(group[e+8:]))
		}
		sizes = append(sizes, s)
	}
	for _, s := range IconSizes {
		if !slices.Contains(sizes, s) {
			return sizes, fmt.Errorf("the icon has no %d px image", s)
		}
	}
	return sizes, nil
}

// rsrc reads a resource section loaded at the virtual address base.
type rsrc struct {
	raw  []byte
	base uint32
}

func (r rsrc) u32(at int) (uint32, error) {
	if at < 0 || at+4 > len(r.raw) {
		return 0, errors.New("the resource section is cut short")
	}
	return binary.LittleEndian.Uint32(r.raw[at:]), nil
}

// child finds the entry with the id in the directory at dir, and returns
// its offset and whether it is a directory.
func (r rsrc) child(dir int, id uint32) (int, bool, error) {
	if dir+16 > len(r.raw) {
		return 0, false, errors.New("the resource section is cut short")
	}
	named := int(binary.LittleEndian.Uint16(r.raw[dir+12:]))
	ids := int(binary.LittleEndian.Uint16(r.raw[dir+14:]))
	for i := named; i < named+ids; i++ {
		e := dir + 16 + 8*i
		name, err := r.u32(e)
		if err != nil {
			return 0, false, err
		}
		if id != 0 && name != id {
			continue
		}
		off, err := r.u32(e + 4)
		if err != nil {
			return 0, false, err
		}
		return int(off &^ 0x80000000), off&0x80000000 != 0, nil
	}
	return 0, false, errors.New("not found")
}

// lookup returns the data of the first language of a resource.
func (r rsrc) lookup(typ, id uint16) ([]byte, error) {
	at, isDir, err := r.child(0, uint32(typ))
	if err != nil || !isDir {
		return nil, errors.New("not found")
	}
	if at, isDir, err = r.child(at, uint32(id)); err != nil || !isDir {
		return nil, errors.New("not found")
	}
	if at, isDir, err = r.child(at, 0); err != nil || isDir {
		return nil, errors.New("no language entry")
	}
	rva, err := r.u32(at)
	if err != nil {
		return nil, err
	}
	size, err := r.u32(at + 4)
	if err != nil {
		return nil, err
	}
	start := int(rva) - int(r.base)
	if start < 0 || start+int(size) > len(r.raw) {
		return nil, errors.New("the data is outside the resource section")
	}
	return r.raw[start : start+int(size)], nil
}
