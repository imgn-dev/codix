require "set"

LIMIT = 10

module Billing
  class Cart < Base
    def initialize(owner, items = [])
      @owner = owner
      @items = items
      @count = 0
    end

    def add(item)
      @items << item
      @count += 1
      total(@count)
    end

    def total(extra = 0)
      sum = extra
      @items.each do |item|
        sum += item.price
      end
      label = describe
      sum + LIMIT + label.size
    end

    def describe
      owner = @owner
      "#{owner} #{self.total}"
    end
  end
end

def build
  cart = Billing::Cart.new("me")
  cart.add(1)
rescue StandardError => error
  raise error
end
