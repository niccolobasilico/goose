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
  var recoveryByDateKey: [String: Double] = [:]

  @Environment(\.dismiss) private var dismiss
  @State private var displayedMonth = Date()
  private let calendar = Calendar.current
  private let columns = Array(repeating: GridItem(.flexible(), spacing: 6), count: 7)

  private static let dateKeyFormatter: DateFormatter = {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.dateFormat = "yyyy-MM-dd"
    return formatter
  }()

  var body: some View {
    VStack(spacing: 0) {
      sheetHeader
        .padding(.horizontal, 18)
        .padding(.top, 18)
        .padding(.bottom, 8)

      monthNavigator
        .padding(.horizontal, 18)
        .padding(.bottom, 12)

      weekdayHeader
        .padding(.horizontal, 18)

      LazyVGrid(columns: columns, alignment: .center, spacing: 10) {
        ForEach(0..<leadingBlankCount, id: \.self) { index in
          Color.clear
            .frame(height: 40)
            .accessibilityHidden(true)
            .id("blank-\(index)")
        }
        ForEach(daysInDisplayedMonth, id: \.self) { date in
          dayCell(for: date)
        }
      }
      .padding(.horizontal, 18)
      .padding(.top, 8)

      legend
        .padding(.top, 18)

      Spacer(minLength: 0)
    }
    .goosePlainBackground()
    .presentationDetents([.medium, .large])
    .presentationDragIndicator(.hidden)
    .onAppear {
      displayedMonth = monthStart(of: selectedDate)
    }
  }

  private var sheetHeader: some View {
    ZStack {
      Text(title)
        .font(.headline.weight(.semibold))
        .fontDesign(.rounded)
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

  private var monthNavigator: some View {
    HStack {
      Button {
        displayedMonth = calendar.date(byAdding: .month, value: -1, to: displayedMonth) ?? displayedMonth
      } label: {
        Image(systemName: "chevron.left")
          .font(.headline.weight(.semibold))
          .frame(width: 38, height: 38)
          .background(.quaternary, in: Circle())
      }
      .buttonStyle(.plain)
      .disabled(!canGoToPreviousMonth)
      .opacity(canGoToPreviousMonth ? 1 : 0.3)

      Spacer()

      Text(displayedMonth.formatted(.dateTime.month(.wide).year()))
        .font(.title3.bold())
        .fontDesign(.rounded)

      Spacer()

      Button {
        displayedMonth = calendar.date(byAdding: .month, value: 1, to: displayedMonth) ?? displayedMonth
      } label: {
        Image(systemName: "chevron.right")
          .font(.headline.weight(.semibold))
          .frame(width: 38, height: 38)
          .background(.quaternary, in: Circle())
      }
      .buttonStyle(.plain)
      .disabled(!canGoToNextMonth)
      .opacity(canGoToNextMonth ? 1 : 0.3)
    }
  }

  private var weekdayHeader: some View {
    let symbols = calendar.veryShortStandaloneWeekdaySymbols
    let ordered = Array(symbols[(calendar.firstWeekday - 1)...] + symbols[..<(calendar.firstWeekday - 1)])
    return LazyVGrid(columns: columns, alignment: .center, spacing: 0) {
      ForEach(Array(ordered.enumerated()), id: \.offset) { _, symbol in
        Text(symbol)
          .font(.caption2.weight(.semibold))
          .foregroundStyle(.secondary)
      }
    }
  }

  @ViewBuilder
  private func dayCell(for date: Date) -> some View {
    let today = calendar.startOfDay(for: Date())
    let day = calendar.startOfDay(for: date)
    let isFuture = day > today
    let isSelected = calendar.isDate(day, inSameDayAs: selectedDate)
    let isToday = calendar.isDate(day, inSameDayAs: today)
    let score = recoveryByDateKey[Self.dateKeyFormatter.string(from: day)]

    Button {
      selectedDate = day
      dismiss()
    } label: {
      Text("\(calendar.component(.day, from: day))")
        .font(.callout.weight(.semibold))
        .fontDesign(.rounded)
        .frame(width: 38, height: 38)
        .background(
          Circle().fill(scoreColor(score).opacity(score == nil ? 0 : 0.30))
        )
        .overlay(
          Circle().strokeBorder(
            isSelected ? Color.pink : (isToday ? Color.secondary.opacity(0.7) : Color.clear),
            lineWidth: 2
          )
        )
        .opacity(isFuture ? 0.25 : 1)
    }
    .buttonStyle(.plain)
    .disabled(isFuture)
    .accessibilityLabel(date.formatted(.dateTime.month().day()))
  }

  private var legend: some View {
    HStack(spacing: 14) {
      legendDot(color: .green, label: "67-100")
      legendDot(color: .yellow, label: "34-66")
      legendDot(color: .red, label: "1-33")
    }
    .font(.caption2)
    .foregroundStyle(.secondary)
  }

  private func legendDot(color: Color, label: String) -> some View {
    HStack(spacing: 5) {
      Circle().fill(color.opacity(0.5)).frame(width: 9, height: 9)
      Text(label)
    }
  }

  private func scoreColor(_ score: Double?) -> Color {
    guard let score else {
      return .clear
    }
    if score >= 67 {
      return .green
    }
    if score >= 34 {
      return .yellow
    }
    return .red
  }

  private func monthStart(of date: Date) -> Date {
    calendar.dateInterval(of: .month, for: date)?.start ?? calendar.startOfDay(for: date)
  }

  private var canGoToPreviousMonth: Bool {
    guard let previous = calendar.date(byAdding: .month, value: -1, to: displayedMonth) else {
      return false
    }
    return previous >= monthStart(of: HealthDataStore.importedHistoryFloor)
  }

  private var canGoToNextMonth: Bool {
    guard let next = calendar.date(byAdding: .month, value: 1, to: displayedMonth) else {
      return false
    }
    return next <= monthStart(of: Date())
  }

  private var leadingBlankCount: Int {
    let firstWeekday = calendar.component(.weekday, from: displayedMonth)
    return (firstWeekday - calendar.firstWeekday + 7) % 7
  }

  private var daysInDisplayedMonth: [Date] {
    guard let range = calendar.range(of: .day, in: .month, for: displayedMonth) else {
      return []
    }
    return range.compactMap { day in
      calendar.date(byAdding: .day, value: day - 1, to: displayedMonth)
    }
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
