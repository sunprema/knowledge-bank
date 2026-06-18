import SwiftUI

struct SidebarView: View {
    @Binding var section: AppSection

    var body: some View {
        List(selection: $section) {
            Section {
                ForEach(AppSection.allCases) { item in
                    Label {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(item.title).font(.body)
                            Text(item.subtitle)
                                .font(.caption2)
                                .foregroundStyle(.tertiary)
                        }
                    } icon: {
                        Image(systemName: item.icon)
                            .foregroundStyle(.tint)
                    }
                    .tag(item)
                    .padding(.vertical, 2)
                }
            } header: {
                Text("Knowledge Bank")
            }
        }
        .listStyle(.sidebar)
        .navigationTitle("KB")
    }
}
