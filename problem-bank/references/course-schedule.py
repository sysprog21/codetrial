def canFinish(numCourses, prerequisites):
    after = [[] for _ in range(numCourses)]
    waiting = [0] * numCourses
    for course, prerequisite in prerequisites:
        after[prerequisite].append(course)
        waiting[course] += 1
    ready = [c for c in range(numCourses) if waiting[c] == 0]
    taken = 0
    while ready:
        course = ready.pop()
        taken += 1
        for nxt in after[course]:
            waiting[nxt] -= 1
            if waiting[nxt] == 0:
                ready.append(nxt)
    return taken == numCourses
