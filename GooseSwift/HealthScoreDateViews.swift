import Darwin
import Foundation
import SwiftUI
import UIKit

struct ScoreDateTitleButton: View {
  let title: String
  let subtitle: String?
  let action: () -> Void

  var body: some View {
    Button(action: action) {
      VStack(spacing: 1) {
        HStack(spacing: 5) {
          Text(title)
            .font(subtitle == nil ? .headline : .subheadline.weight(.semibold))
          Image(systemName: "chevron.down")
            .font(.caption.weight(.bold))
            .baselineOffset(-1)
        }
        if let subtitle {
          Text(subtitle)
            .font(.caption2.weight(.medium))
            .foregroundStyle(.secondary)
        }
      }
      .fontDesign(.rounded)
      .foregroundStyle(.primary)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(subtitle.map { "\(title), \($0)" } ?? title)
    .accessibilityHint("Opens date picker")
  }
}

struct ScoreDatePickerSheet: View {
  let title: String
  let routes: [HealthRoute]
  let snapshots: [HealthMetricSnapshot]
  @Binding var selectedDate: Date

  @Environment(\.dismiss) private var dismiss
  private let calendar = Calendar.current

  var body: some View {
    VStack(spacing: 0) {
      sheetHeader
        .padding(.horizontal, 18)
        .padding(.top, 18)
        .padding(.bottom, 10)

      ScrollView {
        LazyVStack(alignment: .leading, spacing: 30) {
          ForEach(monthStarts, id: \.self) { monthStart in
            ScoreDateMonthSection(
              monthStart: monthStart,
              routes: routes,
              snapshots: snapshots,
              selectedDate: $selectedDate,
              calendar: calendar,
              selectDate: { date in
                selectedDate = date
                dismiss()
              }
            )
          }
        }
        .padding(.horizontal, 18)
        .padding(.bottom, 28)
      }
    }
    .goosePlainBackground()
    .presentationDetents([.large])
    .presentationDragIndicator(.hidden)
  }

  private var sheetHeader: some View {
    ZStack {
      VStack(spacing: 2) {
        Text(title)
          .font(.headline.weight(.semibold))
          .fontDesign(.rounded)
        Text(selectedDate.formatted(.dateTime.month(.wide).year()))
          .font(.subheadline.weight(.medium))
          .foregroundStyle(.secondary)
      }
      HStack {
        Spacer()
        Button {
          dismiss()
        } label: {
          Image(systemName: "xmark")
            .font(.headline.weight(.semibold))
            .frame(width: 42, height: 42)
            .background(.quaternary, in: Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Close")
      }
    }
  }

  private var monthStarts: [Date] {
    let current = calendar.dateInterval(of: .month, for: selectedDate)?.start
      ?? calendar.startOfDay(for: selectedDate)
    // Mesi navigabili fino all'inizio dello storico importato (2023-07).
    let floor = HealthDataStore.importedHistoryFloor
    var months: [Date] = []
    var cursor = current
    while cursor >= floor, months.count < 48 {
      months.append(cursor)
      guard let previous = calendar.date(byAdding: .month, value: -1, to: cursor) else {
        break
      }
      cursor = previous
    }
    if months.isEmpty {
      months.append(current)
    }
    return months
  }
}

struct ScoreDateMonthSection: View {
  let monthStart: Date
  let routes: [HealthRoute]
  let snapshots: [HealthMetricSnapshot]
  @Binding var selectedDate: Date
  let calendar: Calendar
  let selectDate: (Date) -> Void

  private let columns = Array(repeating: GridItem(.flexible(), spacing: 8), count: 7)

  var body: some View {
    VStack(alignment: .leading, spacing: 14) {
      Text(monthStart.formatted(.dateTime.month(.wide)))
        .font(.title2.bold())
        .fontDesign(.rounded)

      LazyVGrid(columns: columns, alignment: .center, spacing: 14) {
        ForEach(0..<leadingBlankCount, id: \.self) { index in
          Color.clear
            .frame(height: 74)
            .accessibilityHidden(true)
            .id("blank-\(index)")
        }

        ForEach(daysInMonth, id: \.self) { date in
          let entry = ScoreDateTimeline.entry(
            for: date,
            routes: routes,
            snapshots: snapshots,
            calendar: calendar
          )
          ScoreDateCell(
            entry: entry,
            isSelected: calendar.isDate(date, inSameDayAs: selectedDate)
          ) {
            selectDate(date)
          }
          .disabled(entry.isFuture)
        }
      }
    }
  }

  private var leadingBlankCount: Int {
    let firstWeekday = calendar.component(.weekday, from: monthStart)
    return (firstWeekday - calendar.firstWeekday + 7) % 7
  }

  private var daysInMonth: [Date] {
    guard let range = calendar.range(of: .day, in: .month, for: monthStart) else {
      return []
    }
    return range.compactMap { day in
      calendar.date(byAdding: .day, value: day - 1, to: monthStart)
    }
  }
}

struct ScoreDateCell: View {
  let entry: ScoreDateEntry
  let isSelected: Bool
  let select: () -> Void

  private var dayNumber: String {
    "\(Calendar.current.component(.day, from: entry.date))"
  }

  var body: some View {
    Button(action: select) {
      VStack(spacing: 6) {
        Text(dayNumber)
          .font(.callout.weight(.semibold))
          .fontDesign(.rounded)
          .foregroundStyle(isSelected ? .white : .primary)
          .frame(width: 30, height: 30)
          .background(selectionBackground)

        ScoreRingStack(metrics: entry.metrics, size: 42)
          .opacity(entry.isFuture ? 0.3 : 1)
      }
      .frame(maxWidth: .infinity, minHeight: 74)
      .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .accessibilityLabel(accessibilityLabel)
  }

  @ViewBuilder
  private var selectionBackground: some View {
    if isSelected {
      Circle().fill(Color.pink)
    }
  }

  private var accessibilityLabel: String {
    let scores = entry.metrics
      .map { "\($0.route.title) \($0.score)" }
      .joined(separator: ", ")
    return "\(entry.date.formatted(.dateTime.month().day())), \(scores)"
  }
}

struct ScoreRingStack: View {
  let metrics: [ScoreDateMetric]
  let size: CGFloat

  var body: some View {
    ZStack {
      ForEach(Array(metrics.prefix(3).enumerated()), id: \.offset) { index, metric in
        let inset = CGFloat(index) * 8
        Circle()
          .stroke(metric.tint.opacity(0.18), lineWidth: 5)
          .frame(width: size - inset, height: size - inset)
        Circle()
          .trim(from: 0, to: CGFloat(metric.score) / 100)
          .stroke(
            metric.tint,
            style: StrokeStyle(lineWidth: 5, lineCap: .round)
          )
          .rotationEffect(.degrees(-90))
          .frame(width: size - inset, height: size - inset)
      }
    }
    .frame(width: size, height: size)
  }
}
