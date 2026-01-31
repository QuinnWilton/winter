# Example Datalog Queries

Winter uses Souffl\u00e9 datalog for reasoning over its knowledge base. Here are some example queries you can run with the `query_facts` tool.

## Basic Queries

### Find all mutual follows
```datalog
mutual_follow(X, Y)
```

### Find people interested in a specific topic
```datalog
interested_in(Person, "rust")
```

### Find all recorded relationships with a specific person
```datalog
follows("did:plc:self", Person)
```

## Derived Queries

These queries use rules you've defined to derive new facts.

### Find potential conversation partners
```datalog
potential_conversation(Person, Topic)
```

This finds people who:
- You mutually follow
- Share an interest with you

### Find shared interests with someone
```datalog
shared_interest("did:plc:self", "did:plc:friend", Topic)
```

## Ad-hoc Queries

You can also write custom datalog for one-off analysis.

### Find people you follow but who don't follow back
```datalog
.decl one_way_follow(x:symbol)
one_way_follow(Y) :- follows("did:plc:self", Y), !follows(Y, "did:plc:self").
.output one_way_follow
```

### Find topics mentioned by multiple people
```datalog
.decl popular_topic(topic:symbol, count:number)
popular_topic(T, count) :- interested_in(_, T), count = count : interested_in(_, T).
.output popular_topic
```

### Find conversation chains
```datalog
.decl replied_chain(a:symbol, b:symbol, c:symbol)
replied_chain(A, B, C) :- replied(A, B), replied(B, C).
.output replied_chain
```

## Common Predicates

Here are the standard predicates Winter uses in its fact base:

| Predicate | Arguments | Description |
|-----------|-----------|-------------|
| `follows(X, Y)` | DIDs | X follows Y |
| `interested_in(X, Topic)` | DID, string | X is interested in Topic |
| `mentioned(X, Topic, Date)` | DID, string, date | X mentioned Topic on Date |
| `replied(X, Y)` | DIDs | X replied to Y |
| `liked(X, Post)` | DID, URI | X liked Post |

## Best Practices

1. **Use DIDs, not handles**: DIDs are stable; handles can change
2. **Keep predicates simple**: One relationship per predicate
3. **Use rules for derived facts**: Don't duplicate logic in queries
4. **Add confidence scores**: Track how certain you are about facts
5. **Tag facts for organization**: Use tags to categorize and filter
