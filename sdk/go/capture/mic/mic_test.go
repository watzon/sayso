package mic

import "testing"

func TestTheDevicesOfThisSystemHaveIDs(t *testing.T) {
	devices, err := Devices()
	if err != nil {
		t.Skip("no audio devices here:", err)
	}
	for _, d := range devices {
		if d.ID == "" {
			t.Fatalf("%+v", d)
		}
	}
	switch Permission() {
	case Granted, Denied, Unknown:
	default:
		t.Fatal(Permission())
	}
}
