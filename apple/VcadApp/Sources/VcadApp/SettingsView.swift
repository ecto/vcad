import SwiftUI

#if os(macOS)
/// vcad ▸ Settings… (⌘,). Every preference here is read by the model through
/// `Prefs`, so the two cannot drift.
struct SettingsView: View {
    @AppStorage(Prefs.presentationKey) private var opensInWindow = false
    @AppStorage(Prefs.gridKey) private var showsGrid = true
    @AppStorage(Prefs.triadKey) private var showsTriad = true
    @AppStorage(Prefs.soundsKey) private var playsSounds = true
    @AppStorage(Prefs.hapticsKey) private var haptics = true

    var body: some View {
        TabView {
            Form {
                Section {
                    Picker("Open new documents:", selection: $opensInWindow) {
                        Text("Released over the desktop").tag(false)
                        Text("In a window").tag(true)
                    }
                    .pickerStyle(.radioGroup)
                } footer: {
                    Text("Released documents float over your desktop with their own Dock tile. Switch any time with View ▸ Open in Window (⇧⌘D).")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Section("Viewport") {
                    Toggle("Show reference grid and contact shadow in windows", isOn: $showsGrid)
                    Toggle("Show orientation triad", isOn: $showsTriad)
                }
            }
            .formStyle(.grouped)
            .tabItem { Label("General", systemImage: "gearshape") }

            Form {
                Section("Feedback") {
                    Toggle("Play sounds when geometry solves or a check changes", isOn: $playsSounds)
                    Toggle("Haptic detents while scrubbing values", isOn: $haptics)
                }
            }
            .formStyle(.grouped)
            .tabItem { Label("Feedback", systemImage: "speaker.wave.2") }
        }
        .frame(width: 480)
        .padding(.top, Theme.Space.s)
    }
}
#endif
