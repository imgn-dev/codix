# frozen_string_literal: true

VERSION = "1.0"

module Acme
  module Billing
    MAX_LINES = 100

    class Invoice < Base
      attr_reader :lines

      def initialize(lines)
        @lines = lines
      end

      def total
        lines.sum(&:amount)
      end

      def self.empty
        new([])
      end

      class << self
        def build(*lines) = new(lines)
      end

      alias sum total

      private

      def secret; end
    end
  end
end

def report(invoice)
  invoice.total.to_s
end
